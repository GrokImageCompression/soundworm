//! macOS HAL bindings via `coreaudio-sys`.
//!
//! This module is the only part of the crate compiled on macOS targets.
//! The actual HAL calls are deliberately left as `TODO(v0.5-mac)`
//! stubs returning `Ok(empty)` / no-op — they need to be filled in on a
//! macOS dev box where the resulting code can actually be exercised.
//!
//! The structural pieces are real and ready:
//!   * `Inner::start()` brings up the HAL listener thread + event
//!     broadcaster, mirroring the `pipewire-backend` shape.
//!   * `device_id_to_node_id` / `node_id_to_device_id` settle the
//!     `AudioDeviceID` (u32) ↔ `NodeId` (u64) mapping.
//!   * `hal_devices()` is where `kAudioHardwarePropertyDevices` will be
//!     called once the bindings are wired.
//!
//! When this is filled in, the call graph should be:
//!
//!   start()
//!     ↳ spawn listener thread
//!         ↳ AudioObjectAddPropertyListener(devices)
//!         ↳ AudioObjectAddPropertyListener(default output)
//!         ↳ CFRunLoopRun()
//!     ↳ initial enumerate via hal_devices()
//!
//!   set_default_output(node_id)
//!     ↳ AudioObjectSetPropertyData(
//!           kAudioObjectSystemObject,
//!           kAudioHardwarePropertyDefaultOutputDevice,
//!           &device_id)
//!
//!   set_volume(node_id, v)
//!     ↳ AudioObjectSetPropertyData(
//!           device_id,
//!           kAudioDevicePropertyVolumeScalar)

use coreaudio_sys::{
    kAudioDevicePropertyDeviceName, kAudioDevicePropertyNominalSampleRate,
    kAudioDevicePropertyStreams, kAudioHardwarePropertyDefaultOutputDevice,
    kAudioHardwarePropertyDevices, kAudioObjectPropertyScopeGlobal,
    kAudioObjectPropertyScopeInput, kAudioObjectPropertyScopeOutput,
    kAudioObjectSystemObject, AudioDeviceID, AudioObjectGetPropertyData,
    AudioObjectGetPropertyDataSize, AudioObjectID, AudioObjectPropertyAddress,
    AudioObjectSetPropertyData, AudioStreamID,
};
use soundworm_core::{
    error::{Result, SoundwormError},
    event::BackendEvent,
    node::{Node, NodeId, NodeKind},
};
use std::collections::HashMap;
use std::os::raw::c_void;
use std::sync::{mpsc, Arc, Mutex};
use std::{mem, ptr};

// Master/Main both equal 0; hardcoding sidesteps the constant rename
// across macOS SDK versions that coreaudio-sys bindgen tracks.
const ELEMENT_MAIN: u32 = 0;

fn address(selector: u32, scope: u32) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress { mSelector: selector, mScope: scope, mElement: ELEMENT_MAIN }
}

/// Byte size the HAL reports for a property, or None on error.
unsafe fn property_size(obj: AudioObjectID, addr: &AudioObjectPropertyAddress) -> Option<usize> {
    let mut size: u32 = 0;
    let st = AudioObjectGetPropertyDataSize(obj, addr, 0, ptr::null(), &mut size);
    (st == 0).then_some(size as usize)
}

/// Read a property whose payload is a packed array of `u32`-sized ids
/// (device ids, stream ids). Returns empty on any HAL error.
unsafe fn read_u32_array(obj: AudioObjectID, addr: &AudioObjectPropertyAddress) -> Vec<u32> {
    let Some(size) = property_size(obj, addr) else { return Vec::new() };
    let count = size / mem::size_of::<u32>();
    if count == 0 {
        return Vec::new();
    }
    let mut buf = vec![0u32; count];
    let mut io = size as u32;
    let st = AudioObjectGetPropertyData(
        obj, addr, 0, ptr::null(), &mut io, buf.as_mut_ptr() as *mut c_void,
    );
    if st != 0 {
        return Vec::new();
    }
    buf.truncate(io as usize / mem::size_of::<u32>());
    buf
}

unsafe fn stream_count(dev: AudioDeviceID, scope: u32) -> usize {
    let addr = address(kAudioDevicePropertyStreams, scope);
    property_size(dev, &addr)
        .map(|s| s / mem::size_of::<AudioStreamID>())
        .unwrap_or(0)
}

unsafe fn device_name(dev: AudioDeviceID) -> String {
    let addr = address(kAudioDevicePropertyDeviceName, kAudioObjectPropertyScopeGlobal);
    let Some(size) = property_size(dev, &addr) else { return String::new() };
    let mut buf = vec![0u8; size];
    let mut io = size as u32;
    let st = AudioObjectGetPropertyData(
        dev, &addr, 0, ptr::null(), &mut io, buf.as_mut_ptr() as *mut c_void,
    );
    if st != 0 {
        return String::new();
    }
    match std::ffi::CStr::from_bytes_until_nul(&buf) {
        Ok(c) => c.to_string_lossy().into_owned(),
        Err(_) => String::new(),
    }
}

unsafe fn nominal_sample_rate(dev: AudioDeviceID) -> u32 {
    let addr = address(kAudioDevicePropertyNominalSampleRate, kAudioObjectPropertyScopeGlobal);
    let mut sr: f64 = 0.0;
    let mut io = mem::size_of::<f64>() as u32;
    let st = AudioObjectGetPropertyData(
        dev, &addr, 0, ptr::null(), &mut io, &mut sr as *mut f64 as *mut c_void,
    );
    if st == 0 && sr > 0.0 {
        sr as u32
    } else {
        48000
    }
}

/// Enumerate the HAL device list into cross-platform `Node`s. A device
/// with output streams is a Sink, with input streams a Source; devices
/// with neither (aggregate control-only) are skipped.
fn hal_devices() -> Vec<Node> {
    unsafe {
        let sys = kAudioObjectSystemObject as AudioObjectID;
        let addr = address(kAudioHardwarePropertyDevices, kAudioObjectPropertyScopeGlobal);
        let devices = read_u32_array(sys, &addr);
        devices
            .into_iter()
            .filter_map(|dev| {
                let outputs = stream_count(dev, kAudioObjectPropertyScopeOutput);
                let inputs = stream_count(dev, kAudioObjectPropertyScopeInput);
                let (kind, media_class) = if outputs > 0 {
                    (NodeKind::Sink, "Audio/Sink")
                } else if inputs > 0 {
                    (NodeKind::Source, "Audio/Source")
                } else {
                    return None;
                };
                Some(Node {
                    id: device_id_to_node_id(dev),
                    name: device_name(dev),
                    kind,
                    app_name: None,
                    media_class: media_class.into(),
                    sample_rate: nominal_sample_rate(dev),
                    channels: 2,
                    latency_ms: 0.0,
                    properties: HashMap::new(),
                })
            })
            .collect()
    }
}

/// `AudioDeviceID` is a `u32` in CoreAudio. We widen to `u64` for
/// `NodeId` to match the cross-platform graph schema; the inverse
/// conversion truncates with a safety check.
pub(crate) fn device_id_to_node_id(dev: u32) -> NodeId {
    NodeId(u64::from(dev))
}

pub(crate) fn node_id_to_device_id(id: u64) -> std::result::Result<u32, SoundwormError> {
    u32::try_from(id).map_err(|_| {
        SoundwormError::Backend(format!("node id {id} out of range for AudioDeviceID"))
    })
}

pub(crate) struct Inner {
    event_sinks: Arc<Mutex<Vec<mpsc::SyncSender<BackendEvent>>>>,
}

impl Inner {
    pub fn start() -> Result<Self> {
        // TODO(v0.5-mac): spawn a HAL listener thread (CFRunLoopRun +
        // AudioObjectAddPropertyListener on kAudioHardwarePropertyDevices)
        // to broadcast live NodeAppeared/NodeRemoved. enumerate_nodes
        // already queries the HAL directly, so listing works without it.
        Ok(Self { event_sinks: Arc::new(Mutex::new(Vec::new())) })
    }

    pub fn subscribe(&self) -> mpsc::Receiver<BackendEvent> {
        let (tx, rx) = mpsc::sync_channel(256);
        self.event_sinks.lock().unwrap().push(tx);
        rx
    }

    pub fn enumerate_nodes(&self) -> Result<Vec<Node>> {
        Ok(hal_devices())
    }

    /// CoreAudio has no port-to-port linking, so "route to this sink"
    /// means make it the system default output device.
    pub fn set_default_output(&self, node_id: u64) -> Result<()> {
        let mut dev = node_id_to_device_id(node_id)?;
        let addr = address(
            kAudioHardwarePropertyDefaultOutputDevice,
            kAudioObjectPropertyScopeGlobal,
        );
        let st = unsafe {
            AudioObjectSetPropertyData(
                kAudioObjectSystemObject as AudioObjectID,
                &addr,
                0,
                ptr::null(),
                mem::size_of::<AudioDeviceID>() as u32,
                &mut dev as *mut AudioDeviceID as *const c_void,
            )
        };
        if st != 0 {
            return Err(SoundwormError::Backend(format!(
                "set default output device {dev} failed: OSStatus {st}"
            )));
        }
        Ok(())
    }

    pub fn set_volume(&self, node_id: u64, volume: f32) -> Result<()> {
        let dev = node_id_to_device_id(node_id)?;
        let v = volume.clamp(0.0, 1.0);
        // TODO(v0.5-mac): walk the output streams of `dev` and
        // AudioObjectSetPropertyData(kAudioDevicePropertyVolumeScalar)
        // on each. Some devices only expose master volume.
        tracing::info!("coreaudio set_volume device={dev} volume={v} (stub)");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_id_roundtrips() {
        let id = device_id_to_node_id(42);
        assert_eq!(node_id_to_device_id(id.0).unwrap(), 42);
    }

    #[test]
    fn device_id_overflow_is_caught() {
        let too_big = u64::from(u32::MAX) + 1;
        assert!(node_id_to_device_id(too_big).is_err());
    }
}
