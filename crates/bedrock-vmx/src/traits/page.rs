#[cfg(not(feature = "cargo"))]
pub use crate::memory::HostPhysAddr;
#[cfg(not(feature = "cargo"))]
use crate::memory::VirtAddr;

#[cfg(feature = "cargo")]
pub use memory::HostPhysAddr;
#[cfg(feature = "cargo")]
use memory::VirtAddr;

pub trait Page {
    /// Returns the page's physical address.
    fn physical_address(&self) -> HostPhysAddr;
    /// Returns the page's virtual address.
    fn virtual_address(&self) -> VirtAddr;
}
