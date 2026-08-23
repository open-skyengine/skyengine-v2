pub mod arm;
pub mod display;
pub mod error;
pub mod mr;
pub mod package;
pub mod runtime;

pub(crate) const VIRTUAL_IMEI: &[u8; 15] = b"000000000000000";
pub(crate) const VIRTUAL_IMSI: &[u8; 15] = b"460019707327302";

pub use display::{DisplayEvent, Framebuffer, PlatformDisplay};
pub use error::{Error, Result};
pub use package::{Package, PackageEntry, PackageHeader, ResourceLimits};
pub use runtime::{DeviceDate, DnsMapping, Runtime, RuntimeConfig, RuntimeState};
