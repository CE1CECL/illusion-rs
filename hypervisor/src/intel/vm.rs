use {
    crate::{
        error::HypervisorError,
        intel::{
            capture::GuestRegisters,
            descriptor::Descriptors,
            page::Page,
            paging::PageTables,
            shared_data::SharedData,
            support::{vmclear, vmptrld, vmread},
            vmcs::Vmcs,
            vmerror::{VmInstructionError, VmxBasicExitReason},
            vmlaunch::launch_vm,
        },
    },
    alloc::boxed::Box,
    bit_field::BitField,
    core::ptr::NonNull,
    x86::{bits64::rflags::RFlags, vmx::vmcs},
};

pub struct Vm {
    /// The VMCS (Virtual Machine Control Structure) for the VM.
    pub vmcs_region: Box<Vmcs>,

    /// The guest's descriptor tables.
    pub guest_descriptor: Descriptors,

    /// The host's descriptor tables.
    pub host_descriptor: Descriptors,

    /// The host's paging tables.
    pub host_paging: Box<PageTables>,

    /// The guest's general-purpose registers state.
    pub guest_registers: GuestRegisters,

    /// The shared data between processors.
    pub shared_data: NonNull<SharedData>,

    /// The MSR bitmaps.
    pub msr_bitmap: Box<Page>,

    /// Whether the VM has been launched.
    pub has_launched: bool,
}

impl Vm {
    pub fn new(guest_registers: &GuestRegisters, shared_data: &mut SharedData) -> Self {
        log::debug!("Creating VM");

        let vmcs_region = Box::new(Vmcs::default());
        let guest_descriptor_table = Descriptors::new_from_current();
        let host_descriptor_table = Descriptors::new_for_host();
        let mut host_paging = unsafe { Box::<PageTables>::new_zeroed().assume_init() };

        host_paging.build_identity();

        let msr_bitmaps = unsafe { Box::<Page>::new_zeroed().assume_init() };
        let has_launched = false;

        log::debug!("VM created");

        Self {
            vmcs_region,
            host_paging,
            host_descriptor: host_descriptor_table,
            guest_descriptor: guest_descriptor_table,
            guest_registers: guest_registers.clone(),
            shared_data: unsafe { NonNull::new_unchecked(shared_data as *mut _) },
            msr_bitmap: msr_bitmaps,
            has_launched,
        }
    }

    pub fn activate_vmcs(&mut self) -> Result<(), HypervisorError> {
        log::debug!("Activating VMCS");
        self.vmcs_region.revision_id.set_bit(31, false);

        // Clear the VMCS region.
        vmclear(self.vmcs_region.as_ref() as *const _ as _);
        log::trace!("VMCLEAR successful!");

        // Load current VMCS pointer.
        vmptrld(self.vmcs_region.as_ref() as *const _ as _);
        log::trace!("VMPTRLD successful!");

        self.setup_vmcs()?;

        log::debug!("VMCS activated successfully!");

        Ok(())
    }

    pub fn setup_vmcs(&mut self) -> Result<(), HypervisorError> {
        log::debug!("Setting up VMCS");

        Vmcs::setup_guest_registers_state(&self.guest_descriptor, &self.guest_registers);
        Vmcs::setup_host_registers_state(&self.host_descriptor, &self.host_paging)?;
        Vmcs::setup_vmcs_control_fields(&mut self.shared_data, &self.msr_bitmap)?;

        log::debug!("VMCS setup successfully!");

        Ok(())
    }

    // launches in a loop returns the types of vmexits
    pub fn run(&mut self) -> Result<VmxBasicExitReason, HypervisorError> {
        // Run the VM until the VM-exit occurs.
        let flags = unsafe { launch_vm(&mut self.guest_registers, u64::from(self.has_launched)) };
        Self::vm_succeed(RFlags::from_raw(flags))?;
        self.has_launched = true;

        // VM-exit occurred. Copy the guest register values from VMCS so that
        // `self.registers` is complete and up to date.
        self.guest_registers.rip = vmread(vmcs::guest::RIP);
        self.guest_registers.rsp = vmread(vmcs::guest::RSP);
        self.guest_registers.rflags = vmread(vmcs::guest::RFLAGS);

        let exit_reason = vmread(vmcs::ro::EXIT_REASON) as u32;

        let Some(basic_exit_reason) = VmxBasicExitReason::from_u32(exit_reason) else {
            log::error!("Unknown exit reason: {:#x}", exit_reason);
            return Err(HypervisorError::UnknownVMExitReason);
        };

        return Ok(basic_exit_reason);
    }

    /// Verifies that the `launch_vm` function executed successfully.
    ///
    /// This method checks the RFlags for indications of failure from the `launch_vm` function.
    /// If a failure is detected, it will panic with a detailed error message.
    ///
    /// # Arguments
    ///
    /// * `flags`: The RFlags value post-execution of the `launch_vm` function.
    ///
    /// Reference: Intel® 64 and IA-32 Architectures Software Developer's Manual:
    /// - 31.2 CONVENTIONS
    /// - 31.4 VM INSTRUCTION ERROR NUMBERS
    fn vm_succeed(flags: RFlags) -> Result<(), HypervisorError> {
        if flags.contains(RFlags::FLAGS_ZF) {
            let instruction_error = vmread(vmcs::ro::VM_INSTRUCTION_ERROR) as u32;
            return match VmInstructionError::from_u32(instruction_error) {
                Some(error) => {
                    log::error!("VM instruction error: {:?}", error);
                    Err(HypervisorError::VmInstructionError)
                }
                None => {
                    log::error!("Unknown VM instruction error: {:#x}", instruction_error);
                    Err(HypervisorError::UnknownVMInstructionError)
                }
            };
        } else if flags.contains(RFlags::FLAGS_CF) {
            log::error!("VM instruction failed due to carry flag being set");
            return Err(HypervisorError::VMFailToLaunch);
        }

        Ok(())
    }
}
