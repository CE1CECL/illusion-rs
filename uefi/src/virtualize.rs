//! Provides a mechanism to virtualize the system by installing a hypervisor on the current processor,
//! utilizing custom stack allocation and low-level assembly for context switching. Essential for
//! enabling hardware-assisted virtualization with specific guest register configurations.
//! Credits to Satoshi Tanda: https://github.com/tandasat/Hello-VT-rp/blob/main/hypervisor/src/switch_stack.rs

use {
    alloc::alloc::handle_alloc_error,
    core::{alloc::Layout, arch::global_asm},
    hypervisor::{
        allocator::allocate_host_stack,
        global_const::STACK_NUMBER_OF_PAGES,
        intel::{capture::GuestRegisters, page::Page},
        vmm::start_hypervisor,
    },
    log::debug,
};

/// Installs the hypervisor on the current processor.
///
/// # Arguments
///
/// * `guest_registers` - The guest registers to use for the hypervisor.
pub fn virtualize_system(guest_registers: &GuestRegisters) -> ! {
    debug!("Allocating stack space for host");

    let host_stack = allocate_host_stack() as usize;
    debug!("Stack range: {:#x?}", host_stack..STACK_NUMBER_OF_PAGES);

    unsafe { switch_stack(guest_registers, start_hypervisor as usize, host_stack as _) };
}

extern "efiapi" {
    /// Jumps to the landing code with the new stack pointer.
    fn switch_stack(guest_registers: &GuestRegisters, landing_code: usize, host_stack: u64) -> !;
}

global_asm!(
    r#"
// The module containing the `switch_stack` function. Jumps to the landing code with the new stack pointer.
.global switch_stack
switch_stack:
    xchg    bx, bx
    mov     rsp, r8
    jmp     rdx
"#
);
