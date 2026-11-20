/*++
Voice Studio Virtual Microphone - user-mode audio injection.

The app (AI Voice Studio) opens \\.\VoiceStudioMic and pushes processed
48 kHz mono float frames (10 ms each) via IOCTL_VSM_PUSH_AUDIO. The capture
miniport pulls from this ring when filling its WaveRT buffer, converting to
the negotiated pin format.
--*/

#pragma once

// pull the same prelude as every other TU in this driver
#include "definitions.h"
#include <wdmsec.h>

#define IOCTL_VSM_GET_VERSION CTL_CODE(FILE_DEVICE_UNKNOWN, 0x800, METHOD_BUFFERED, FILE_ANY_ACCESS)
#define IOCTL_VSM_PUSH_AUDIO  CTL_CODE(FILE_DEVICE_UNKNOWN, 0x801, METHOD_BUFFERED, FILE_ANY_ACCESS)
#define IOCTL_VSM_FLUSH       CTL_CODE(FILE_DEVICE_UNKNOWN, 0x802, METHOD_BUFFERED, FILE_ANY_ACCESS)

#define VSM_PROTOCOL_VERSION 1

NTSTATUS VsmCreateControlDevice(VOID);

// capture-side interface
VOID VsmResetStream();
VOID VsmReadSamples(
    _Out_writes_bytes_(ByteCount) BYTE* Dst,
    _In_ ULONG ByteCount,
    _In_ PKSDATAFORMAT_WAVEFORMATEXTENSIBLE Format
);
