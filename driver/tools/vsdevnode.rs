// vsdevnode — creates the ROOT\VoiceStudioMic device node so PnP installs
// our driver (equivalent of `devcon install VoiceStudioMic.inf ROOT\VoiceStudioMic`).
// Run elevated. Usage: vsdevnode.exe
#![windows_subsystem = "windows"]

use std::os::windows::ffi::OsStrExt;
use std::path::Path;

// SwDeviceCreate (Swdevice.dll, Windows 8+)
#[link(name = "Swdevice")]
extern "system" {
    fn SwDeviceCreate(
        enumerator: *const u16,
        parent: *const u16,
        property_keys: *const std::ffi::c_void,
        cPropertyKeys: u32,
        device_instance_id: *const u16,
        flags: u32,
        lifetime: u32,
        callback: *const std::ffi::c_void,
        context: *const std::ffi::c_void,
        pszDeviceInstanceId: *mut u16,
        cDeviceInstanceId: u32,
    ) -> i32;
}

const SW_DEVICE_CREATE_ALLOW_DUAL_INSTANCE: u32 = 0x4;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn main() {
    // requires the INF to be staged first (pnputil /add-driver ... /install)
    let enumerator = wide("");
    let parent = wide("HTREE\\ROOT\\0");
    let id = wide("ROOT\\VoiceStudioMic");

    // HW keys that tell PnP which driver package matches
    let mut props: Vec<u8> = Vec::new();
    // DEVPROP_TYPE_STRING = 0x12: {A45C254E-DF1C-4EFD-8020-67D146A850E0},2 DeviceDesc
    let mut push_str_prop = |key_guid: [u8; 16], pid: u32, value: &str| {
        props.extend_from_slice(&key_guid);
        props.extend_from_slice(&pid.to_le_bytes());
        props.extend_from_slice(&0x12u32.to_le_bytes()); // DEVPROP_TYPE_STRING
        let w = wide(value);
        let bytes: Vec<u8> = w.iter().flat_map(|c| c.to_le_bytes()).collect();
        props.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        props.extend_from_slice(&bytes);
    };
    push_str_prop(
        [
            0x4e, 0xa2, 0x25, 0xa4, 0x1c, 0xdf, 0xfd, 0x4e, 0x80, 0x20, 0x67, 0xd1, 0x46, 0xa8,
            0x50, 0xe0,
        ],
        2,
        "Voice Studio Virtual Microphone",
    );

    let mut instance = [0u16; 200];
    let st = unsafe {
        SwDeviceCreate(
            enumerator.as_ptr(),
            parent.as_ptr(),
            props.as_ptr() as *const _,
            1,
            id.as_ptr(),
            SW_DEVICE_CREATE_ALLOW_DUAL_INSTANCE,
            0, // lifetime: until reboot/delete
            std::ptr::null(),
            std::ptr::null(),
            instance.as_mut_ptr(),
            200,
        )
    };
    if st == 0 {
        println!("Voice Studio virtual microphone device created.");
    } else {
        eprintln!("SwDeviceCreate failed: HRESULT 0x{:08X}", st as u32);
        std::process::exit(1);
    }
    let _ = Path::new("C:\\Windows\\System32").exists();
}
