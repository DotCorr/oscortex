//! Architecture-neutral legacy I/O port facade.

cfg_if::cfg_if! {
    if #[cfg(target_arch = "x86_64")] {
        pub use crate::arch::x86_64::port_io::*;
    } else {
        pub unsafe fn inb(_port: u16) -> u8 {
            0
        }
        pub unsafe fn outb(_port: u16, _val: u8) {}
        pub unsafe fn inw(_port: u16) -> u16 {
            0
        }
        pub unsafe fn outw(_port: u16, _val: u16) {}
        pub unsafe fn inl(_port: u16) -> u32 {
            0
        }
        pub unsafe fn outl(_port: u16, _val: u32) {}
    }
}
