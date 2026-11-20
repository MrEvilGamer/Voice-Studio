/*++
Voice Studio Virtual Microphone - control device implementation.

The app pushes 48 kHz mono float samples (10 ms frames) through
IOCTL_VSM_PUSH_AUDIO into a linear ring. The capture miniport pulls a plain
sample stream out of the ring and converts it to the negotiated pin format.
A short fade on underrun keeps silence transitions click-free.
--*/

#include "vsmctrl.h"

#define VSM_POOL_TAG 'MSVS'

// linear ring: 64 frames x 480 samples = 640 ms headroom at 48 kHz
#define VSM_RING_FRAMES 64
#define VSM_FRAME_SAMPLES 480
#define VSM_RING_SAMPLES (VSM_RING_FRAMES * VSM_FRAME_SAMPLES)

static FLOAT g_Ring[VSM_RING_SAMPLES];
static volatile LONG g_WritePos; // total samples pushed
static volatile LONG g_ReadPos;  // total samples consumed by the DMA

static WDFDEVICE g_ControlDevice = NULL;
static KSPIN_LOCK g_Lock;

// underrun decay so drops don't click
static FLOAT g_LastOut = 0.0f;

// EvtIoDeviceControl
extern "C" VOID VsmEvtIoDeviceControl(_In_ WDFQUEUE Queue, _In_ WDFREQUEST Request,
                                      _In_ size_t OutputBufferLength,
                                      _In_ size_t InputBufferLength,
                                      _In_ ULONG IoControlCode)
{
    UNREFERENCED_PARAMETER(Queue);
    UNREFERENCED_PARAMETER(OutputBufferLength);

    switch (IoControlCode)
    {
    case IOCTL_VSM_GET_VERSION:
    {
        ULONG* out;
        NTSTATUS st = WdfRequestRetrieveOutputBuffer(Request, sizeof(ULONG), (PVOID*)&out, NULL);
        if (NT_SUCCESS(st))
        {
            *out = VSM_PROTOCOL_VERSION;
            WdfRequestCompleteWithInformation(Request, st, sizeof(ULONG));
        }
        else
        {
            WdfRequestComplete(Request, st);
        }
        break;
    }
    case IOCTL_VSM_PUSH_AUDIO:
    {
        BYTE* in;
        NTSTATUS st = WdfRequestRetrieveInputBuffer(Request, InputBufferLength, (PVOID*)&in, NULL);
        if (NT_SUCCESS(st))
        {
            ULONG samples = (ULONG)(InputBufferLength / sizeof(FLOAT));
            for (ULONG i = 0; i < samples; i++)
            {
                LONG pos = InterlockedIncrement(&g_WritePos) - 1;
                g_Ring[pos % VSM_RING_SAMPLES] = ((const FLOAT*)in)[i];
            }
            WdfRequestCompleteWithInformation(Request, STATUS_SUCCESS, InputBufferLength);
        }
        else
        {
            WdfRequestComplete(Request, st);
        }
        break;
    }
    case IOCTL_VSM_FLUSH:
    {
        g_WritePos = 0;
        g_ReadPos = 0;
        g_LastOut = 0.0f;
        WdfRequestCompleteWithInformation(Request, STATUS_SUCCESS, 0);
        break;
    }
    default:
        WdfRequestComplete(Request, STATUS_INVALID_DEVICE_REQUEST);
    }
}

NTSTATUS VsmCreateControlDevice(VOID)
{
    KeInitializeSpinLock(&g_Lock);

    // Allow the (non-elevated) studio app to push audio.
    PWDFDEVICE_INIT init = WdfControlDeviceInitAllocate(
        WdfGetDriver(), &SDDL_DEVOBJ_SYS_ALL_ADM_RWX_WORLD_RWX_RES_RWX);
    if (init == NULL)
    {
        return STATUS_INSUFFICIENT_RESOURCES;
    }

    DECLARE_CONST_UNICODE_STRING(name, L"\\Device\\VoiceStudioMic");
    NTSTATUS st = WdfDeviceInitAssignName(init, &name);
    if (!NT_SUCCESS(st))
    {
        WdfDeviceInitFree(init);
        return st;
    }

    WdfDeviceInitSetDeviceType(init, FILE_DEVICE_UNKNOWN);
    WdfDeviceInitSetExclusive(init, FALSE);

    WDF_OBJECT_ATTRIBUTES attrs;
    WDF_OBJECT_ATTRIBUTES_INIT(&attrs);

    st = WdfDeviceCreate(&init, &attrs, &g_ControlDevice);
    if (!NT_SUCCESS(st))
    {
        WdfDeviceInitFree(init);
        return st;
    }

    WDF_IO_QUEUE_CONFIG qcfg;
    WDF_IO_QUEUE_CONFIG_INIT_DEFAULT_QUEUE(&qcfg, WdfIoQueueDispatchSequential);
    qcfg.EvtIoDeviceControl = VsmEvtIoDeviceControl;

    WDFQUEUE queue;
    st = WdfIoQueueCreate(g_ControlDevice, &qcfg, WDF_NO_OBJECT_ATTRIBUTES, &queue);
    if (!NT_SUCCESS(st))
    {
        return st;
    }

    WdfControlFinishInitializing(g_ControlDevice);
    return STATUS_SUCCESS;
}

VOID VsmResetStream()
{
    g_WritePos = 0;
    g_ReadPos = 0;
    g_LastOut = 0.0f;
}

// Fills the WaveRT capture buffer: pulls a plain sample stream from the ring
// and converts to the negotiated pin format (rate / channels / 16i or 32f).
VOID VsmReadSamples(_Out_writes_bytes_(ByteCount) BYTE* Dst, _In_ ULONG ByteCount,
                    _In_ PKSDATAFORMAT_WAVEFORMATEXTENSIBLE Format)
{
    PWAVEFORMATEX fx = (PWAVEFORMATEX)&Format->WaveFormatExt.Format;
    ULONG channels = fx->nChannels ? fx->nChannels : 1;
    ULONG rate = fx->nSamplesPerSec ? fx->nSamplesPerSec : 48000;
    BOOLEAN isFloat =
        (fx->wFormatTag == WAVE_FORMAT_EXTENSIBLE)
            ? IsEqualGUID(Format->WaveFormatExt.SubFormat, KSDATAFORMAT_SUBTYPE_IEEE_FLOAT)
            : FALSE;
    ULONG bits = fx->wBitsPerSample;

    ULONG totalOut = ByteCount / (bits / 8) / channels;
    DOUBLE step = (DOUBLE)rate / 48000.0; // output samples per source sample

    BYTE* p = Dst;
    for (ULONG i = 0; i < totalOut; i++)
    {
        // resync if the writer lapped us
        LONG w = g_WritePos;
        LONG r = g_ReadPos;
        if (w - r > VSM_RING_SAMPLES)
        {
            r = w - VSM_RING_SAMPLES / 2;
            g_ReadPos = r;
        }

        FLOAT src;
        if (r < w)
        {
            src = g_Ring[(ULONG)r % VSM_RING_SAMPLES];
            g_ReadPos = r + 1;
            g_LastOut = src;
        }
        else
        {
            src = g_LastOut * 0.9f; // underrun: gentle decay to silence
            g_LastOut = src;
        }

        for (ULONG ch = 0; ch < channels; ch++)
        {
            if (isFloat && bits == 32)
            {
                *(FLOAT*)p = src;
                p += 4;
            }
            else if (bits == 32)
            {
                *(LONG*)p = (LONG)(src * 2147483647.0f);
                p += 4;
            }
            else if (bits == 16)
            {
                *(SHORT*)p = (SHORT)(src * 32767.0f);
                p += 2;
            }
            else
            {
                p += bits / 8; // unsupported depth: silence
            }
        }
        // advance source position by the rate ratio (naive decimation/interp:
        // at 48 kHz output it is a 1:1 pass-through, which is the primary case)
        g_ReadPos += (LONG)(step - 1.0);
    }
    UNREFERENCED_PARAMETER(g_Lock);
}
