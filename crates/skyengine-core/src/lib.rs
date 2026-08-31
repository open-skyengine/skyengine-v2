pub mod arm;
pub mod audio;
pub mod display;
pub mod error;
pub mod mr;
pub mod package;
pub mod runtime;

mod wap_proxy;

pub(crate) const VIRTUAL_IMEI: &[u8; 15] = b"000000000000000";
pub(crate) const VIRTUAL_IMSI: &[u8; 15] = b"460019707327302";

pub use audio::{
    AUDIO_CHANNELS, AUDIO_SAMPLE_RATE, AudioPlayer, PlatformAudio, SilentAudio, SoundType,
};
pub use display::{DisplayEvent, Framebuffer, PlatformDisplay};
pub use error::{Error, Result};
pub use package::{Package, PackageEntry, PackageHeader, ResourceLimits};
pub use runtime::{
    DEFAULT_MEMORY_LIMIT, DeviceDate, DnsMapping, Runtime, RuntimeConfig, RuntimeState,
};
