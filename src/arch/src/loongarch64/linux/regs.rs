use kvm_bindings::{KVM_REG_LOONGARCH, KVM_REG_SIZE_U64, LOONGARCH_REG_SHIFT};
use kvm_ioctls::VcpuFd;
use log::debug;
use std::arch::asm;
use std::result;
const KVM_REG_LOONGARCH_CPUCFG: u64 = (KVM_REG_LOONGARCH as u64) | 0x40000;
const CPUCFG0_REG_ID: u64 =
    KVM_REG_LOONGARCH_CPUCFG | KVM_REG_SIZE_U64 | (0_u64 << LOONGARCH_REG_SHIFT);
const CPUCFG1_KVM_MASK: u64 = (1u64 << 26) - 1;

const CPUCFG2_FP: u64 = 1 << 0;
const CPUCFG2_FPSP: u64 = 1 << 1;
const CPUCFG2_FPDP: u64 = 1 << 2;
const CPUCFG2_FPVERS: u64 = 0x7 << 3;
const CPUCFG2_LSX: u64 = 1 << 6;
const CPUCFG2_LASX: u64 = 1 << 7;
const CPUCFG2_LLFTP: u64 = 1 << 14;
const CPUCFG2_LLFTPREV: u64 = 0x7 << 15;
const CPUCFG2_LSPW: u64 = 1 << 21;
const CPUCFG2_LAM: u64 = 1 << 22;

const CPUCFG3_KVM_MASK: u64 = (1u64 << 17) - 1;
const CPUCFG4_KVM_MASK: u64 = 0xffff_ffff;
const CPUCFG5_KVM_MASK: u64 = 0xffff_ffff;

const CPUCFG16_CACHE_CONFIG: u64 = 0xF;  // L1I, L1D, L2, L3 present
const CPUCFG17_L1I_MASK: u64 = ((5u64) << 24) | ((8u64) << 16) | ((4u64 - 1) << 0);
const CPUCFG18_L1D_MASK: u64 = ((5u64) << 24) | ((8u64) << 16) | ((4u64 - 1) << 0);
const CPUCFG19_L2_MASK: u64 = ((6u64) << 24) | ((9u64) << 16) | ((8u64 - 1) << 0);
const CPUCFG20_L3_MASK: u64 = ((6u64) << 24) | ((10u64) << 16) | ((16u64 - 1) << 0);

#[derive(Debug)]
pub enum Error {
    GetCoreRegs(kvm_ioctls::Error),
    SetCoreRegs(kvm_ioctls::Error),
    SetOneReg(kvm_ioctls::Error),
}

type Result<T> = result::Result<T, Error>;

pub fn setup_regs(
    vcpu: &VcpuFd,
    boot_ip: u64,
    cmdline_addr: u64,
    efi_boot: bool,
    system_table: u64,
) -> Result<()> {
    setup_cpucfg(vcpu)?;
    let mut regs = vcpu.get_regs().map_err(Error::GetCoreRegs)?;
    regs.pc = boot_ip;
    regs.gpr[4] = u64::from(efi_boot);
    regs.gpr[5] = cmdline_addr;
    regs.gpr[6] = system_table;

    debug!(
        "loongarch setup_regs: pc=0x{:x}, a0={}, a1=0x{:x}, a2=0x{:x}",
        regs.pc, regs.gpr[4], regs.gpr[5], regs.gpr[6],
    );
    let mut cpucfg0 = [0_u8; 8];
    if vcpu.get_one_reg(CPUCFG0_REG_ID, &mut cpucfg0).is_ok() {
        let cpucfg0 = u64::from_le_bytes(cpucfg0);
        debug!("loongarch cpucfg0: 0x{:x}", cpucfg0);
    }
    vcpu.set_regs(&regs).map_err(Error::SetCoreRegs)?;
    Ok(())
}

#[inline]
fn cpucfg_reg_id(index: u64) -> u64 {
    KVM_REG_LOONGARCH_CPUCFG | KVM_REG_SIZE_U64 | (index << LOONGARCH_REG_SHIFT)
}

#[inline]
fn read_host_cpucfg(index: u64) -> u64 {
    let value: u64;
    unsafe {
        asm!(
            "cpucfg {value}, {index}",
            value = out(reg) value,
            index = in(reg) index,
        );
    }
    value
}

#[inline]
fn filter_cpucfg_for_kvm(index: u64, host_value: u64) -> u64 {
    match index {
        0 => host_value & 0xffff_ffff,
        1 => host_value & CPUCFG1_KVM_MASK,
        2 => {
            let mut mask = CPUCFG2_FP
                | CPUCFG2_FPSP
                | CPUCFG2_FPDP
                | CPUCFG2_FPVERS
                | CPUCFG2_LLFTP
                | CPUCFG2_LLFTPREV
                | CPUCFG2_LSPW
                | CPUCFG2_LAM;

            if host_value & CPUCFG2_LSX != 0 {
                mask |= CPUCFG2_LSX;
            }
            if host_value & CPUCFG2_LASX != 0 {
                mask |= CPUCFG2_LASX;
            }

            host_value & mask
        }
        3 => host_value & CPUCFG3_KVM_MASK,
        4 => host_value & CPUCFG4_KVM_MASK,
        5 => host_value & CPUCFG5_KVM_MASK,
        _ => 0,
    }
}

fn setup_cpucfg(vcpu: &VcpuFd) -> Result<()> {
    // Setup CPUCFG 0-5: Basic CPU information
    for index in 0..=5u64 {
        let host_value = read_host_cpucfg(index);
        let guest_value = filter_cpucfg_for_kvm(index, host_value);

        vcpu.set_one_reg(cpucfg_reg_id(index), &guest_value.to_le_bytes())
            .map_err(Error::SetOneReg)?;

        debug!(
            "loongarch set cpucfg{}: host=0x{:x}, guest=0x{:x}",
            index, host_value, guest_value,
        );
    }

    // Setup CPUCFG 16-20: Cache configuration
    // CPUCFG16: Cache configuration (which cache levels exist)
    // Format: L1I present | L1D present | L2 present | L3 present
    let cpucfg16 = CPUCFG16_CACHE_CONFIG;
    vcpu.set_one_reg(cpucfg_reg_id(16), &cpucfg16.to_le_bytes())
        .map_err(Error::SetOneReg)?;
    debug!("loongarch set cpucfg16: 0x{:x}", cpucfg16);
    // CPUCFG17-20: Cache properties for each level
    // Format: ways[13:8] | sets[21:16] | linesz[15:12] | other[11:0]
    // We'll use common values for a typical Loongson 3A5000-like CPU
    vcpu.set_one_reg(cpucfg_reg_id(17), &CPUCFG17_L1I_MASK.to_le_bytes())
        .map_err(Error::SetOneReg)?;
    vcpu.set_one_reg(cpucfg_reg_id(18), &CPUCFG18_L1D_MASK.to_le_bytes())
        .map_err(Error::SetOneReg)?;
    vcpu.set_one_reg(cpucfg_reg_id(19), &CPUCFG19_L2_MASK.to_le_bytes())
        .map_err(Error::SetOneReg)?;
    vcpu.set_one_reg(cpucfg_reg_id(20), &CPUCFG20_L3_MASK.to_le_bytes())
        .map_err(Error::SetOneReg)?;
    debug!(
        "loongarch set cpucfg17-20: 0x{:x}, 0x{:x}, 0x{:x}, 0x{:x}",
        CPUCFG17_L1I_MASK, CPUCFG18_L1D_MASK, CPUCFG19_L2_MASK, CPUCFG20_L3_MASK
    );

    Ok(())
}
