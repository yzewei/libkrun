# LoongArch64 KVM 启动完整函数调用链

本文档以 GDB 调用栈的格式，详细记录 libkrun 在 LoongArch64 架构下启动 KVM 虚拟机的完整流程。

## 测试环境

- **命令**: `./smoke_kernel /home/yzw/python-trans/linux/arch/loongarch/boot/vmlinux.efi ./rootfs_debian_unstable`
- **内核**: Linux 6.9.0 (LoongArch64)
- **RootFS**: Debian Unstable (virtiofs)
- **日期**: 2026 年 3 月 24 日

补充说明：

- 本文档最初整理于早期 bring-up 阶段，当时主要围绕 `vmlinuz.efi` / `PE+GZ` 路径调试
- 当前代码已经支持：
  - `PE + gzip`
  - plain `vmlinux.efi`
- 当前 `/home/yzw/python-trans/linux` 这棵树的稳定产物是 `arch/loongarch/boot/vmlinux.efi`
- 当前用户态调试流程也已经推进到：
  - `ttyS0` 负责早期日志
  - `hvc0` 负责交互 shell

## 调用链路总览

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                           用户空间 (Userspace - libkrun)                     │
├─────────────────────────────────────────────────────────────────────────────┤
│  libkrun API 层 (src/libkrun/src/lib.rs)                                    │
│    └── krun_run_vm()                      # 启动虚拟机（C API 入口）          │
│          └── build_microvm()              # 构建 microVM 主函数               │
│                ├── choose_payload()         # 选择启动载荷类型（内核/固件）   │
│                ├── create_guest_memory()    # 创建客户机内存映射              │
│                ├── setup_vm()               # 创建 KVM VM 实例                │
│                │     ├── KvmContext::new()    # 打开/dev/kvm 设备文件         │
│                │     ├── Vm::new()            # 创建 VM 文件描述符            │
│                │     └── Vm::memory_init()    # 注册内存区域到 KVM            │
│                ├── load_external_kernel()   # 加载外部内核文件               │
│                │     └── [加载 PE/GZ 或 plain PE 格式内核]                    │
│                ├── create_vcpus_loongarch64() # 创建 LoongArch vCPUs         │
│                │     ├── Vcpu::new_loongarch64() # 创建单个 vCPU             │
│                │     └── Vcpu::configure_loongarch64() # 配置 vCPU 寄存器     │
│                │           ├── setup_cpucfg()   # 初始化 CPU 特性寄存器       │
│                │           └── setup_regs()     # 设置通用寄存器 (PC/a0-a2)  │
│                ├── Vcpu::try_clone_irq_vcpu_file() # 克隆 irq 专用 vCPU fd   │
│                ├── KvmLoongArchIrqChip::new() # 创建中断控制器               │
│                │     ├── KVM_DEV_TYPE_LOONGARCH_IPI    # IPI 设备            │
│                │     ├── KVM_DEV_TYPE_LOONGARCH_EIOINTC # EIOINTC 设备       │
│                │     ├── KVM_DEV_TYPE_LOONGARCH_PCHPIC  # PCHPIC 设备        │
│                │     └── irq_vcpu_fd -> KVM_INTERRUPT    # 主中断注入路径    │
│                ├── attach_legacy_devices()  # 附加传统设备（串口等）         │
│                │     └── register_mmio_serial() # 注册 MMIO 串口             │
│                ├── attach_virtio_devices()  # 附加 virtio 设备               │
│                │     ├── attach_balloon_device() # 附加内存气球设备          │
│                │     ├── attach_rng_device() # 附加随机数生成器              │
│                │     ├── attach_console_devices() # 附加控制台设备           │
│                │     └── attach_fs_devices() # 附加文件系统设备              │
│                ├── Vmm::configure_system()  # 配置系统硬件                   │
│                │     ├── fdt::loongarch64::create_fdt() # 创建设备树 FDT     │
│                │     │     ├── create_cpu_nodes()       # 创建 CPU 节点       │
│                │     │     ├── create_memory_node()     # 创建内存节点       │
│                │     │     ├── create_chosen_node()     # 创建启动参数节点   │
│                │     │     ├── create_cpuintc_node()    # 创建 CPU 中断控制器 │
│                │     │     ├── create_eiointc_node()    # 创建 EIOINTC 节点   │
│                │     │     ├── create_pic_node()        # 创建 PCHPIC 节点    │
│                │     │     └── create_devices_node()    # 创建设备节点       │
│                │     │           ├── create_serial_node() # 创建串口节点     │
│                │     │           └── create_virtio_node() # 创建 virtio 节点  │
│                │     └── arch::loongarch64::configure_system() # 架构配置    │
│                │           └── efi::setup_fdt_system_table() # 创建 EFI 表    │
│                └── Vmm::start_vcpus()       # 启动所有 vCPU 线程             │
│                      ├── Vcpu::register_kick_signal_handler() # 注册唤醒信号 │
│                      └── Vcpu::start_threaded() # 创建 vCPU 运行线程         │
│                            └── Vcpu::run()  # vCPU 主循环                    │
│                                  └── StateMachine::run() # 状态机主循环      │
│                                        └── Vcpu::running() # 运行状态处理    │
│                                              └── Vcpu::run_emulation() # 执行仿真│
│                                                    └── KVM_RUN (ioctl) # 进入 Guest│
├─────────────────────────────────────────────────────────────────────────────┤
│  内核空间 (Kernel Space - KVM)                                               │
├─────────────────────────────────────────────────────────────────────────────┤
│    └── kvm_arch_vcpu_ioctl_run() # KVM vCPU 运行 ioctl                       │
│          ├── vcpu_enter_guest() # 切换到 Guest 模式                           │
│          │     └── 切换到 Guest 模式 (VM Entry)                               │
│          └── 处理 VM Exit               # 处理退出原因                       │
│                ├── MMIO 访问 → mmio_bus.read/write() # 模拟 MMIO 读写         │
│                ├── IOCSR 访问 → LoongArchIocsrState # 模拟 IOCSR 寄存器       │
│                └── KVM_INTERRUPT → kvm_queue_irq() # 注入中断到 vCPU          │
└─────────────────────────────────────────────────────────────────────────────┘
```

## 当前与最初调用链的关键差异

这份文档的主体仍然反映了当前 LoongArch 启动主线，但有 4 个点需要额外记住：

1. 当前不再要求输入文件必须是 `vmlinuz.efi`

LoongArch loader 现在已经支持 plain `vmlinux.efi`。当前 `/home/yzw/python-trans/linux` 这棵树默认可直接喂给 libkrun 的文件就是：

- `arch/loongarch/boot/vmlinux.efi`

2. 当前 `KvmLoongArchIrqChip::new()` 背后不只是“创建 IPI/EIOINTC/PCHPIC”

当前实现还会：

- 从 `vcpus[0]` clone 一个 irq 专用 fd
- 用它驱动 `KVM_INTERRUPT`

3. 当前 serial/virtio 主中断路径已经不是 `irqfd`

LoongArch 分支已经显式跳过：

- serial 的 `register_irqfd`
- virtio 的 `register_irqfd`

当前主路径是：

- serial：`sync_interrupt() -> set_irq_state() -> KVM_INTERRUPT`
- virtio：`try_signal()/ACK -> set_irq_state() -> KVM_INTERRUPT`

4. 当前 bring-up 已经推进到用户态交互阶段

因此，除了这份“启动调用链”之外，后续继续工作时还应同时参考：

- `docs/LOONGARCH_PORTING.md`
- `docs/LOONGARCH_TROUBLESHOOTING_HISTORY.md`
- `docs/LOONGARCH_SMOKE_TEST.md`

---

## 阶段 1：libkrun API 入口

### #0  krun_run_vm
```
文件：src/libkrun/src/lib.rs
行号：~700
```

**作用**: libkrun C API 入口，启动虚拟机

**调用栈**:
```
#0  krun_run_vm (ctx_id=0)           # 启动指定 ID 的虚拟机
    at src/libkrun/src/lib.rs:7xx
```

**关键代码**:
```rust
pub fn krun_run_vm(ctx_id: i32) -> i32 {
    // 获取 Vmm 上下文（根据 ctx_id 查找已创建的 VMM）
    let vmm = get_vmm(ctx_id)?;
    // 启动 VMM（调用 Vmm::run）
    vmm.run()
}
```

**参数说明**:
- `ctx_id`: 虚拟机上下文 ID，由 `krun_create_ctx()` 创建

**返回值**:
- `0`: 成功
- `<0`: 错误码

---

## 阶段 2：VM 构建 (build_microvm)

### #1  build_microvm
```
文件：src/vmm/src/builder.rs
行号：565
```

**作用**: 构建 microVM 的主入口函数，负责创建和配置整个虚拟机环境

**调用栈**:
```
#1  build_microvm (                    # 构建 microVM
        vm_resources: VmResources,     # 输入：VM 资源配置
        event_manager: &mut EventManager, # 输入：事件管理器
    ) -> Result<Arc<Mutex<Vmm>>>       # 输出：VMM 实例（引用计数 + 互斥锁）
    at src/vmm/src/builder.rs:565
```

**输入**:
- `vm_resources`: VM 资源配置（vCPU 数、内存大小、内核路径、设备配置等）
- `event_manager`: 事件管理器（处理 epoll 事件）

**输出**:
- `Arc<Mutex<Vmm>>`: VMM 实例（线程安全的引用计数指针）

**主要流程**:
```rust
pub fn build_microvm(
    vm_resources: &VmResources,
    event_manager: &mut EventManager,
) -> Result<Arc<Mutex<Vmm>>> {
    // 1. 选择载荷类型（内核/固件）
    let payload = choose_payload(vm_resources)?;
    
    // 2. 创建 guest 内存（计算布局并映射）
    let (guest_memory, arch_memory_info) = create_guest_memory(...)?;
    
    // 3. 创建 KVM VM（打开/dev/kvm，创建 VM fd）
    let vm = setup_vm(&guest_memory)?;
    
    // 4. 加载内核（读取并加载到 guest 内存）
    let payload_config = load_payload(&guest_memory, &payload, ...)?;
    
    // 5. 创建 vCPUs (LoongArch64 特定：配置 CPUCFG 和寄存器)
    #[cfg(all(target_arch = "loongarch64", target_os = "linux"))]
    {
        vcpus = create_vcpus_loongarch64(...)?;
    }
    
    // 6. clone 一个 irq 专用 vCPU fd
    let irq_vcpu_fd = vcpus[0].try_clone_irq_vcpu_file()?;

    // 7. 创建中断控制器（IPI、EIOINTC、PCHPIC + KVM_INTERRUPT 主路径）
    intc = KvmLoongArchIrqChip::new(vm.fd(), vcpu_count, irq_vcpu_fd)?;

    // 8. 附加传统设备（串口等 MMIO 设备）
    attach_legacy_devices(...)?;

    // 9. 附加 virtio 设备（balloon、rng、console、fs 等）
    attach_balloon_device(...)?;
    attach_rng_device(...)?;
    attach_console_devices(...)?;
    attach_fs_devices(...)?;
    
    // 10. 配置系统（创建 FDT 设备树，写入 EFI System Table）
    vmm.configure_system(...)?;
    
    // 11. 启动 vCPUs（创建线程，发送 Resume 事件）
    vmm.start_vcpus(...)?;
    
    Ok(vmm)
}
```

---

## 阶段 3：载荷选择与内存创建

### #2  choose_payload
```
文件：src/vmm/src/builder.rs
行号：534
```

**作用**: 确定启动载荷类型（内核映射/内核复制/外部内核/固件）

**调用栈**:
```
#2  choose_payload (
        vm_resources: &VmResources     # 输入：VM 资源配置
    ) -> Result<Payload>               # 输出：载荷类型枚举
    at src/vmm/src/builder.rs:534
```

**LoongArch64 逻辑**:
```rust
// LoongArch64 使用 KernelCopy 模式（需要复制内核到 guest 内存）
#[cfg(any(target_arch = "aarch64", target_arch = "riscv64", target_arch = "loongarch64"))]
return Ok(Payload::KernelCopy);
```

**载荷类型说明**:
- `KernelMmap`: 内核直接映射到 guest 内存（x86_64 专用）
- `KernelCopy`: 内核复制到 guest 内存（ARM64/LoongArch64/RISC-V）
- `ExternalKernel`: 外部提供的内核文件
- `Firmware`: UEFI/BIOS 固件

**当前日志可能出现两种情况**:
```
Found GZIP header on PE file at: ...  # 输入是 PE+GZ
```

或：

```
No GZIP header found on PE file; treating it as plain PE image
```

第二种情况说明当前输入的是 plain `vmlinux.efi`。

---

### #3  create_guest_memory
```
文件：src/vmm/src/builder.rs
行号：303
```

**作用**: 创建并初始化客户机物理内存，计算各区域地址布局

**调用栈**:
```
#3  create_guest_memory (
        memory_size: usize,            # 输入：内存大小（字节）
        initrd_size: u64,              # 输入：initrd 大小
    ) -> Result<(GuestMemoryMmap, ArchMemoryInfo)>
                                       # 输出：内存映射 + 架构内存信息
    at src/vmm/src/builder.rs:303
```

**LoongArch64 内存布局计算**:
```rust
// src/arch/src/loongarch64/mod.rs
pub fn arch_memory_regions(
    size: usize,                       # 请求的内存大小
    initrd_size: u64,                  # initrd 大小
    _firmware_size: Option<usize>,     # 固件大小（未使用）
) -> (ArchMemoryInfo, Vec<(GuestAddress, usize)>) {
    // 1. 获取系统页面大小（通常 4KB）
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    
    // 2. 对齐 DRAM 大小到页面边界
    let dram_size = align_upwards!(size, page_size);
    
    // 3. 计算 RAM 末尾地址（起始地址 + 大小）
    let ram_last_addr = layout::DRAM_MEM_START + (dram_size as u64);
    
    // 4. 计算 SHM 起始地址（按 1GB 对齐）
    let shm_start_addr = ((ram_last_addr / 0x4000_0000) + 1) * 0x4000_0000;

    // 5. 计算各区域地址（从高到低排列）
    let fdt_addr = ram_last_addr - layout::FDT_MAX_SIZE as u64;
    let efi_system_table_addr = fdt_addr - layout::EFI_GUEST_SIZE;
    let initrd_addr = efi_system_table_addr - initrd_size;

    // 6. 构建架构内存信息结构
    let info = ArchMemoryInfo {
        ram_last_addr,              // RAM 末尾地址
        shm_start_addr,             // 共享内存起始地址
        page_size,                  // 页面大小
        fdt_addr,                   // FDT 设备树地址
        efi_system_table_addr,      // EFI 系统表地址
        initrd_addr,                // initrd 地址
        firmware_addr: FIRMWARE_START, // 固件地址（0）
    };
    
    // 7. 定义内存区域（LoongArch 只有一个连续区域）
    let regions = vec![(GuestAddress(layout::DRAM_MEM_START), dram_size)];

    (info, regions)  // 返回内存信息和区域列表
}
```

**内存布局**:
```
地址空间布局 (LoongArch64 Guest)
物理地址              大小         用途
─────────────────────────────────────────────────────────
0x0000_0000_0000_0000
├─────────────────────┐ ← 0x0A00_0000 (MAPPED_IO_START)
│    MMIO 设备区       │   - 串口 0xa001000
│                     │   - PCHPIC 0x10000000
│                     │   - Virtio MMIO 0xa002000~
├─────────────────────┤ ← 0x4000_0000 (DRAM_MEM_START)
│                     │
│       Kernel        │   内核加载地址 0x40200000
│                     │
├─────────────────────┤
│        RAM          │   主内存区域（1GB 起始）
│                     │
├─────────────────────┤ ← ram_last_addr (示例：0xc0000000)
│   initrd (如果有)   │   initrd_addr
├─────────────────────┤
│   cmdline (16KB)    │   cmdline_addr = 0xbffe8000
├─────────────────────┤
│ EFI System Table    │   efi_system_table_addr = 0xbffec000
├─────────────────────┤
│   FDT (设备树 64KB) │   fdt_addr = 0xbfff0000
├─────────────────────┤ ← RAM 末尾
│       保留区        │
├─────────────────────┤ ← shm_start_addr (按 1GB 对齐)
│    共享内存区        │   VirtioFS 共享内存
└─────────────────────┘
```

---

## 阶段 4：KVM VM 创建

### #4  setup_vm
```
文件：src/vmm/src/builder.rs
行号：1677
```

**作用**: 创建 KVM VM 实例（打开/dev/kvm，创建 VM，注册内存）

**调用栈**:
```
#4  setup_vm (
        guest_memory: &GuestMemoryMmap  # 输入：已创建的 guest 内存映射
    ) -> Result<Vm>                     # 输出：VM 实例
    at src/vmm/src/builder.rs:1677
    #5  KvmContext::new ()              # 打开/dev/kvm 设备
        at src/vmm/src/linux/vstate.rs
        #6  open("/dev/kvm", O_RDWR | O_CLOEXEC)  # 系统调用
    #7  Vm::new (
        kvm_fd: &KvmFd                  # 输入：KVM 文件描述符
    ) -> Result<Vm>                     # 输出：VM 实例
        at src/vmm/src/linux/vstate.rs
        #8  kvm_fd.create_vm()          # ioctl: KVM_CREATE_VM
    #9  Vm::memory_init (
        guest_memory: &GuestMemoryMmap  # 输入：guest 内存
    )                                   # 注册内存区域到 KVM
        at src/vmm/src/linux/vstate.rs
        #10 vm_fd.set_user_memory_region()  # ioctl: KVM_SET_USER_MEMORY_REGION
```

**关键代码**:
```rust
fn setup_vm(guest_memory: &GuestMemoryMmap) -> Result<Vm> {
    let kvm = KvmContext::new()?;  // 步骤 1: 打开/dev/kvm 设备文件
    let vm = Vm::new(kvm.fd())?;   // 步骤 2: 创建 VM (ioctl: KVM_CREATE_VM)
    vm.memory_init(guest_memory)?; // 步骤 3: 注册内存区域 (ioctl: KVM_SET_USER_MEMORY_REGION)
    Ok(vm)
}
```

**说明**:
- `KvmContext::new()`: 打开 `/dev/kvm` 获取 KVM 文件描述符
- `Vm::new()`: 通过 `KVM_CREATE_VM` ioctl 创建 VM 文件描述符
- `Vm::memory_init()`: 通过 `KVM_SET_USER_MEMORY_REGION` ioctl 将用户空间内存映射到 guest 物理地址

---

## 阶段 5：内核加载

### #5  load_external_kernel
```
文件：src/vmm/src/builder.rs
行号：1185
```

**作用**: 加载外部内核文件到 guest 内存（支持 PE/GZ 压缩格式）

**调用栈**:
```
#5  load_external_kernel (
        guest_mem: &GuestMemoryMmap,   # 输入：guest 内存映射
        arch_mem_info: &ArchMemoryInfo, # 输入：架构内存信息
        external_kernel: &ExternalKernel, # 输入：内核配置（路径、格式）
    ) -> Result<(GuestAddress, Option<InitrdConfig>, Option<String>)>
                                       # 输出：(入口地址，initrd 配置，命令行)
    at src/vmm/src/builder.rs:1185
```

**LoongArch64 PE/GZ 格式处理**:
```rust
#[cfg(target_arch = "loongarch64")]
KernelFormat::PeGz => {
    // === 常量定义 ===
    const LOONGARCH_IMAGE_HEADER_SIZE: usize = 64;     // LoongArch 内核头部大小
    const LOONGARCH_KERNEL_ENTRY_OFFSET: usize = 8;    // 入口地址偏移
    const LOONGARCH_LOAD_OFFSET_OFFSET: usize = 24;    // 加载偏移偏移
    const LOONGARCH_LINUX_PE_MAGIC_OFFSET: usize = 56; // PE 魔数偏移
    const LOONGARCH_LINUX_PE_MAGIC: u32 = 0x8182_23cd; // LoongArch PE 魔数
    const LOONGARCH_VMLINUX_LOAD_ADDRESS: u64 = 0x9000_0000_0020_0000; // 编译时加载地址

    // 步骤 1: 读取并解压内核（GZIP 解压）
    let data = std::fs::read(external_kernel.path)?;  // 读取内核文件
    let gzip_offset = data.windows(3).position(|w| w == [0x1f, 0x8b, 0x8])?; // 定位 GZIP 头
    let (_, compressed) = data.split_at(gzip_offset);  // 分割出压缩数据
    let mut gz = GzDecoder::new(compressed);           // 创建 GZIP 解码器
    let mut kernel_data = Vec::new();                  // 存储解压后的数据
    gz.read_to_end(&mut kernel_data)?;                 // 解压到内存

    // 步骤 2: 验证 PE 头部（确保是正确的 LoongArch 内核）
    let pe_magic = u32::from_le_bytes(
        kernel_data[LOONGARCH_LINUX_PE_MAGIC_OFFSET..].try_into()?
    );
    if pe_magic != LOONGARCH_LINUX_PE_MAGIC {
        return Err(StartMicrovmError::PeGzInvalid);  // 魔数不匹配，返回错误
    }

    // 步骤 3: 解析入口地址和加载偏移（从内核头部读取）
    let kernel_entry = u64::from_le_bytes(
        kernel_data[LOONGARCH_KERNEL_ENTRY_OFFSET..].try_into()?
    );  // 内核入口虚拟地址
    let load_offset = u64::from_le_bytes(
        kernel_data[LOONGARCH_LOAD_OFFSET_OFFSET..].try_into()?
    );  // 相对于 DRAM_MEM_START 的偏移

    // 步骤 4: 计算实际加载地址和入口地址
    let image_load_addr = GuestAddress(
        arch::loongarch64::layout::DRAM_MEM_START + load_offset
    );  // guest 物理加载地址
    let entry_offset = kernel_entry - LOONGARCH_VMLINUX_LOAD_ADDRESS;  // 入口偏移量
    let entry_addr = GuestAddress(image_load_addr.0 + entry_offset);   // guest 物理入口地址

    // 步骤 5: 写入内存（将内核镜像复制到 guest 内存）
    guest_mem.write(&kernel_data, image_load_addr)?;

    debug!("loongarch pegz image_load_addr=0x{:x}, entry_addr=0x{:x}",
           image_load_addr.0, entry_addr.0);

    entry_addr  // 返回内核入口地址
}
```

**日志输出**:
```
[2026-03-24T03:12:55.453721Z DEBUG vmm::builder]
    loongarch pegz image_load_addr=0x40200000, entry_addr=0x41660000
    # image_load_addr: 内核镜像加载地址 (DRAM_MEM_START + 0x200000)
    # entry_addr: 内核实际入口地址 (0x41660000)
[2026-03-24T03:12:55.466719Z DEBUG vmm::builder]
    load_external_kernel: 0x41660000
```

---

## 阶段 6：vCPU 创建与配置

### #6  create_vcpus_loongarch64
```
文件：src/vmm/src/builder.rs
行号：1995
```

**作用**: 创建并配置所有 LoongArch64 vCPU（批量创建）

**调用栈**:
```
#6  create_vcpus_loongarch64 (
        vm: &Vm,                       # 输入：VM 文件描述符
        vcpu_config: &VcpuConfig,      # 输入：vCPU 配置（数量等）
        entry_addr: GuestAddress,      # 输入：内核入口地址
        cmdline_addr: GuestAddress,    # 输入：命令行地址
        efi_system_table_addr: GuestAddress, # 输入：EFI 系统表地址
        exit_evt: &EventFd,            # 输入：退出事件文件描述符
    ) -> Result<Vec<Vcpu>>             # 输出：vCPU 列表
    at src/vmm/src/builder.rs:1995
```

**关键代码**:
```rust
fn create_vcpus_loongarch64(
    vm: &Vm,
    vcpu_config: &VcpuConfig,
    entry_addr: GuestAddress,
    cmdline_addr: GuestAddress,
    efi_system_table_addr: GuestAddress,
    exit_evt: &EventFd,
) -> Result<Vec<Vcpu>> {
    use arch::loongarch64::linux::iocsr::LoongArchIocsrState;

    // 步骤 1: 创建 IOCSR 状态共享结构（用于模拟 IOCSR 寄存器）
    let iocsr_state = Arc::new(LoongArchIocsrState::new(vcpu_count));
    let mut vcpus = Vec::new();

    // 步骤 2: 循环创建每个 vCPU
    for cpu_index in 0..vcpu_config.vcpu_count {
        // 步骤 2.1: 创建 vCPU（调用 KVM_CREATE_VCPU ioctl）
        let mut vcpu = Vcpu::new_loongarch64(
            cpu_index,              # vCPU ID（0, 1, 2...）
            vm.fd(),                # VM 文件描述符
            exit_evt.try_clone()?,  # 退出事件（克隆一份）
            iocsr_state.clone(),    # IOCSR 状态（共享）
        )?;

        // 步骤 2.2: 配置 vCPU 寄存器（PC、a0-a2 等）
        vcpu.configure_loongarch64(
            vm.fd(),                # VM 文件描述符
            entry_addr,             # 内核入口地址
            cmdline_addr,           # 命令行地址
            efi_system_table_addr   # EFI 系统表地址
        )?;

        vcpus.push(vcpu);  # 添加到列表
    }
    Ok(vcpus)  # 返回 vCPU 列表
}
```

**说明**:
- `iocsr_state`: 共享的 IOCSR 寄存器状态，用于模拟 LoongArch 特有的 CSR 寄存器
- 每个 vCPU 都会克隆 `exit_evt` 用于通知 VM 退出事件
- `configure_loongarch64` 会设置 PC 寄存器、CPUCFG 寄存器、以及 EFI 启动参数

---

### #7  Vcpu::new_loongarch64
```
文件：src/vmm/src/linux/vstate.rs
行号：~950
```

**作用**: 创建单个 LoongArch64 vCPU 实例

**调用栈**:
```
#7  Vcpu::new_loongarch64 (
        id: u8,                        # 输入：vCPU ID（0-based）
        vm_fd: &VmFd,                  # 输入：VM 文件描述符
        exit_evt: EventFd,             # 输入：退出事件文件描述符
        iocsr_state: Arc<LoongArchIocsrState>, # 输入：IOCSR 状态共享指针
    ) -> Result<Vcpu>                  # 输出：vCPU 实例
    at src/vmm/src/linux/vstate.rs:9xx
```

**关键代码**:
```rust
pub fn new_loongarch64(
    id: u8,
    vm_fd: &VmFd,
    exit_evt: EventFd,
    iocsr_state: Arc<LoongArchIocsrState>,
) -> Result<Self> {
    // 步骤 1: 通过 KVM_CREATE_VCPU ioctl 创建 vCPU 文件描述符
    let kvm_vcpu = vm_fd.create_vcpu(id as u64)?;
    
    // 步骤 2: 创建事件通道（用于 vCPU 线程间通信）
    let (event_sender, event_receiver) = unbounded();
    let (response_sender, response_receiver) = unbounded();

    // 步骤 3: 构建 Vcpu 结构体
    Ok(Vcpu {
        fd: kvm_vcpu,                   # vCPU 文件描述符
        id,                             # vCPU ID
        mmio_bus: None,                 # MMIO 总线（初始为空）
        exit_evt,                       # 退出事件
        event_receiver,                 # 事件接收器
        event_sender: Some(event_sender), # 事件发送器
        response_receiver: Some(response_receiver), # 响应接收器
        response_sender,                # 响应发送器
        iocsr_state,                    # IOCSR 状态（LoongArch 特有）
    })
}
```

**字段说明**:
- `fd`: KVM vCPU 文件描述符，用于后续 ioctl 调用
- `event_sender/receiver`: 用于主线程向 vCPU 线程发送事件（Pause/Resume）
- `response_sender/receiver`: 用于 vCPU 线程向主线程返回响应
- `iocsr_state`: LoongArch 特有的 CSR 寄存器共享状态

---

### #8  Vcpu::configure_loongarch64
```
文件：src/vmm/src/linux/vstate.rs
行号：~1000
```

**作用**: 配置 LoongArch64 vCPU 寄存器（PC、CPUCFG、启动参数）

**调用栈**:
```
#8  Vcpu::configure_loongarch64 (
        &mut self,                     # 输入：vCPU 可变引用
        vm_fd: &VmFd,                  # 输入：VM 文件描述符
        kernel_load_addr: GuestAddress, # 输入：内核加载地址
        cmdline_addr: GuestAddress,    # 输入：命令行地址
        efi_system_table_addr: GuestAddress, # 输入：EFI 系统表地址
    ) -> Result<()>                    # 输出：成功/失败
    at src/vmm/src/linux/vstate.rs:10xx
    #9  arch::loongarch64::regs::setup_cpucfg()  # 初始化 CPUCFG 寄存器
    #10 arch::loongarch64::regs::setup_regs()    # 设置通用寄存器
```

**关键代码**:
```rust
pub fn configure_loongarch64(
    &mut self,
    _vm_fd: &VmFd,
    kernel_load_addr: GuestAddress,
    cmdline_addr: GuestAddress,
    efi_system_table_addr: GuestAddress,
) -> Result<()> {
    // 调用架构特定的寄存器配置函数
    arch::loongarch64::regs::setup_regs(
        &self.fd,                       # vCPU 文件描述符
        kernel_load_addr.raw_value(),   # 内核入口地址（PC 寄存器）
        cmdline_addr.raw_value(),       # 命令行地址（a1 寄存器）
        true,                           # efi_boot = true（使用 EFI 启动）
        efi_system_table_addr.raw_value(), # EFI 系统表地址（a2 寄存器）
    )
    .map_err(Error::REGSConfiguration)?;  # 错误转换
    Ok(())
}
```

**说明**:
- 此函数是 LoongArch64 特定的配置函数
- 主要工作是调用 `setup_regs` 设置 vCPU 初始状态
- `efi_boot = true` 表示使用 EFI handoff 方式启动

---

### #9  setup_cpucfg
```
文件：src/arch/src/loongarch64/linux/regs.rs
行号：95
```

**作用**: 初始化 vCPU 的 CPUCFG 寄存器（bring-up 必需，解决 KVM 未初始化问题）

**调用栈**:
```
#9  setup_cpucfg (
        vcpu: &VcpuFd                  # 输入：vCPU 文件描述符
    ) -> Result<()>                    # 输出：成功/失败
    at src/arch/src/loongarch64/linux/regs.rs:95
    #10 read_host_cpucfg(index)        # 内联汇编读取 host CPUCFG
    #11 filter_cpucfg_for_kvm(index, host_value) # 过滤特性位
    #12 vcpu.set_one_reg(reg_id, &value) # 写入 guest vCPU
```

**关键代码**:
```rust
fn setup_cpucfg(vcpu: &VcpuFd) -> Result<()> {
    // 遍历 CPUCFG0-CPUCFG5 寄存器
    for index in 0..=5u64 {
        // 步骤 1: 读取 host CPUCFG 寄存器（使用内联汇编）
        let host_value = read_host_cpucfg(index);

        // 步骤 2: 根据 KVM 要求过滤特性位（隐藏某些主机特性）
        let guest_value = filter_cpucfg_for_kvm(index, host_value);

        // 步骤 3: 写入 guest vCPU（通过 KVM_SET_ONE_REG ioctl）
        vcpu.set_one_reg(cpucfg_reg_id(index), &guest_value.to_le_bytes())?;

        debug!("loongarch set cpucfg{}: host=0x{:x}, guest=0x{:x}",
               index, host_value, guest_value);
    }
    Ok(())  // 所有 CPUCFG 设置完成
}
```

**日志输出**:
```
[2026-03-24T03:12:55.467692Z DEBUG arch::loongarch64::linux::regs]
    loongarch set cpucfg0: host=0x14d010, guest=0x14d010
    # CPUCFG0: CPU 基本信息（厂商、系列）
[2026-03-24T03:12:55.467702Z DEBUG arch::loongarch64::linux::regs]
    loongarch set cpucfg1: host=0x7f2f2fe, guest=0x3f2f2fe
    # CPUCFG1: 过滤后的特性位
[2026-03-24T03:12:55.467707Z DEBUG arch::loongarch64::linux::regs]
    loongarch set cpucfg2: host=0x7f7ccfcf, guest=0x60c0cf
    # CPUCFG2: 向量扩展特性（LSX/LASX）
[2026-03-24T03:12:55.467711Z DEBUG arch::loongarch64::linux::regs]
    loongarch set cpucfg3: host=0xcefcff, guest=0xfcff
    # CPUCFG3: 其他特性
[2026-03-24T03:12:55.467715Z DEBUG arch::loongarch64::linux::regs]
    loongarch set cpucfg4: host=0x5f5e100, guest=0x5f5e100
    # CPUCFG4: 平台特性
[2026-03-24T03:12:55.467719Z DEBUG arch::loongarch64::linux::regs]
    loongarch set cpucfg5: host=0x10001, guest=0x10001
    # CPUCFG5: 扩展特性
```

**为什么需要 setup_cpucfg？**:
- KVM LoongArch 当前不会自动为 Guest 初始化 `cpucfg[]` 寄存器
- 如果不初始化，Guest 内核在早期 `cpu_probe()` 阶段读到 `CPUCFG0=0x0`
- 导致 CPU 厂商/系列识别失败，内核卡在启动早期阶段
- 这是 bring-up required 的临时修复，未来可能迁移到 KVM 内核层

---

### #10  setup_regs
```
文件：src/arch/src/loongarch64/linux/regs.rs
行号：48
```

**作用**: 配置 vCPU 通用寄存器（PC、a0-a2），设置 EFI 启动参数

**调用栈**:
```
#10 setup_regs (
        vcpu: &VcpuFd,                 # 输入：vCPU 文件描述符
        boot_ip: u64,                  # 输入：内核入口地址（PC 寄存器）
        cmdline_addr: u64,             # 输入：命令行地址（a1 寄存器）
        efi_boot: bool,                # 输入：EFI 启动标志（a0 寄存器）
        system_table: u64,             # 输入：EFI 系统表地址（a2 寄存器）
    ) -> Result<()>                    # 输出：成功/失败
    at src/arch/src/loongarch64/linux/regs.rs:48
```

**关键代码**:
```rust
pub fn setup_regs(
    vcpu: &VcpuFd,
    boot_ip: u64,
    cmdline_addr: u64,
    efi_boot: bool,
    system_table: u64
) -> Result<()> {
    // 步骤 1: 先初始化 CPUCFG 寄存器（bring-up 必需）
    setup_cpucfg(vcpu)?;
    
    // 步骤 2: 获取当前寄存器状态（通过 KVM_GET_REGS ioctl）
    let mut regs = vcpu.get_regs()?;
    
    // 步骤 3: 设置关键寄存器
    regs.pc = boot_ip;                      // PC: 程序计数器，指向内核入口
    regs.gpr[4] = u64::from(efi_boot);      // a0: EFI 启动标志（1=EFI）
    regs.gpr[5] = cmdline_addr;             // a1: 内核命令行地址
    regs.gpr[6] = system_table;             // a2: EFI System Table 地址
    
    // 步骤 4: 打印调试信息
    debug!("loongarch setup_regs: pc=0x{:x}, a0={}, a1=0x{:x}, a2=0x{:x}",
           regs.pc, regs.gpr[4], regs.gpr[5], regs.gpr[6]);

    // 步骤 5: 写回寄存器状态（通过 KVM_SET_REGS ioctl）
    vcpu.set_regs(&regs)?;
    Ok(())
}
```

**寄存器说明**（LoongArch 调用约定）:
- `pc`: 程序计数器，CPU 从这里开始执行
- `gpr[4]` (a0): 第一个参数，EFI 启动标志（1 表示使用 EFI）
- `gpr[5]` (a1): 第二个参数，内核命令行地址
- `gpr[6]` (a2): 第三个参数，EFI System Table 地址

**日志输出**:
```
[2026-03-24T03:12:55.467740Z DEBUG arch::loongarch64::linux::regs]
    loongarch setup_regs: pc=0x41660000, a0=1, a1=0xbffe8000, a2=0xbffec000
    # pc=0x41660000: 内核入口地址
    # a0=1: EFI 启动模式
    # a1=0xbffe8000: 命令行地址
    # a2=0xbffec000: EFI System Table 地址
```

---

## 阶段 7：中断控制器创建

### #11  KvmLoongArchIrqChip::new
```
文件：src/devices/src/legacy/kvmloongarchirqchip.rs
行号：18
```

**作用**: 创建 LoongArch 三级中断控制器（IPI、EIOINTC、PCHPIC）

**调用栈**:
```
#11 KvmLoongArchIrqChip::new (
        vm: &VmFd,                     # 输入：VM 文件描述符
        vcpu_count: u32,               # 输入：vCPU 数量
    ) -> Result<KvmLoongArchIrqChip>   # 输出：中断控制器实例
    at src/devices/src/legacy/kvmloongarchirqchip.rs:18
    #12 vm.create_device(KVM_DEV_TYPE_LOONGARCH_IPI)    # 创建 IPI 设备
    #13 vm.create_device(KVM_DEV_TYPE_LOONGARCH_EIOINTC) # 创建 EIOINTC 设备
    #14 vm.create_device(KVM_DEV_TYPE_LOONGARCH_PCHPIC)  # 创建 PCHPIC 设备
```

**关键代码**:
```rust
pub fn new(vm: &VmFd, vcpu_count: u32) -> Result<Self> {
    // 步骤 1: 创建 IPI 设备（处理器间中断）
    let mut ipi_device = kvm_create_device {
        type_: KVM_DEV_TYPE_LOONGARCH_IPI,  # IPI 设备类型
        fd: 0,
        flags: 0,
    };
    let ipi_fd = vm.create_device(&mut ipi_device)?;  # ioctl: KVM_CREATE_DEVICE

    // 步骤 2: 创建 EIOINTC 设备（扩展 I/O 中断控制器）
    let mut eiointc_device = kvm_create_device {
        type_: KVM_DEV_TYPE_LOONGARCH_EIOINTC,  # EIOINTC 设备类型
        fd: 0,
        flags: 0,
    };
    let eiointc_fd = vm.create_device(&mut eiointc_device)?;

    // 步骤 3: 配置 EIOINTC - 设置 CPU 数量
    let nr_cpus = vcpu_count;
    let attr = kvm_device_attr {
        group: KVM_DEV_LOONGARCH_EXTIOI_GRP_CTRL,  # 控制组
        attr: KVM_DEV_LOONGARCH_EXTIOI_CTRL_INIT_NUM_CPU as u64,  # 初始化 CPU 数量
        addr: &nr_cpus as *const u32 as u64,  # 参数地址
        flags: 0,
    };
    eiointc_fd.set_device_attr(&attr)?;  # ioctl: KVM_SET_DEVICE_ATTR

    // 步骤 4: 创建 PCHPIC 设备（PCI 主机中断控制器）
    let mut pchpic_device = kvm_create_device {
        type_: KVM_DEV_TYPE_LOONGARCH_PCHPIC,  # PCHPIC 设备类型
        fd: 0,
        flags: 0,
    };
    let pchpic_fd = vm.create_device(&mut pchpic_device)?;

    // 步骤 5: 配置 PCHPIC 基地址
    let pch_pic_base: u64 = 0x1000_0000;  # PCHPIC MMIO 基地址
    let attr = kvm_device_attr {
        group: KVM_DEV_LOONGARCH_PCH_PIC_GRP_CTRL,  # 控制组
        attr: KVM_DEV_LOONGARCH_PCH_PIC_CTRL_INIT as u64,  # 初始化命令
        addr: &pch_pic_base as *const u64 as u64,  # 基地址参数
        flags: 0,
    };
    pchpic_fd.set_device_attr(&attr)?;

    // 步骤 6: 创建用于 KVM_INTERRUPT 的 vCPU fd
    let irq_vcpu_fd = vm.create_vcpu(0)?;

    Ok(Self {
        _ipi_fd: ipi_fd,
        _eiointc_fd: eiointc_fd,
        _pchpic_fd: pchpic_fd,
        irq_vcpu_fd,  // 用于中断注入
        vcpu_count,
    })
}
```

**三级中断架构说明**:
```
CPU INTC (CPU 中断控制器)
    └── EIOINTC (扩展 I/O 中断控制器)
            └── PCHPIC (PCI 主机中断控制器)
                    ├── 串口
                    └── Virtio 设备
```
        addr: &pch_pic_base as *const u64 as u64,
        flags: 0,
    };
    pchpic_fd.set_device_attr(&attr)?;

    Ok(Self {
        _ipi_fd: ipi_fd,
        _eiointc_fd: eiointc_fd,
        _pchpic_fd: pchpic_fd,
        irq_vcpu_fd: vm.create_vcpu(0)?,  // 用于 KVM_INTERRUPT
        vcpu_count,
    })
}
```

---

## 阶段 8：传统设备附加

### #12  attach_legacy_devices
```
文件：src/vmm/src/builder.rs
行号：1803
```

**作用**: 附加传统设备（串口等）

**调用栈**:
```
#12 attach_legacy_devices (
        vm: &Vm,
        mmio_device_manager: &mut MMIODeviceManager,
        kernel_cmdline: &mut Cmdline,
        intc: IrqChip,
        serial: Vec<Arc<Mutex<Serial>>>,
    ) -> Result<()>
    at src/vmm/src/builder.rs:1803
    #13 register_mmio_serial()
```

**关键代码**:
```rust
fn attach_legacy_devices(
    vm: &Vm,
    mmio_device_manager: &mut MMIODeviceManager,
    kernel_cmdline: &mut Cmdline,
    intc: IrqChip,
    serial: Vec<Arc<Mutex<Serial>>>,
) -> Result<()> {
    for s in serial {
        mmio_device_manager
            .register_mmio_serial(vm.fd(), kernel_cmdline, intc.clone(), s)?;
    }
    Ok(())
}
```

**添加 earlycon 参数**:
```rust
// src/vmm/src/device_manager/kvm/mmio.rs
#[cfg(target_arch = "loongarch64")]
kernel_cmdline.insert("earlycon", 
    &format!("uart8250,mmio,0x{:08x}", mmio_base))?;
```

---

## 阶段 9：Virtio 设备附加

### #13  attach_virtio_devices
```
文件：src/vmm/src/builder.rs
行号：~1030
```

**作用**: 附加 virtio 设备（balloon、rng、console、fs 等）

**调用栈**:
```
#13 attach_virtio_devices
    at src/vmm/src/builder.rs:1030
    #14 attach_balloon_device()
    #15 attach_rng_device()
    #16 attach_console_devices()
    #17 attach_fs_devices()
```

**IRQ 分配日志**:
```
[2026-03-24T03:12:56.094311Z DEBUG devices::virtio::mmio[balloon]] set_irq_line: 3
[2026-03-24T03:12:56.094346Z DEBUG devices::virtio::mmio[rng]] set_irq_line: 4
[2026-03-24T03:12:56.094507Z DEBUG devices::virtio::mmio[console]] set_irq_line: 5
[2026-03-24T03:12:56.094552Z DEBUG devices::virtio::mmio[fs]] set_irq_line: 6
[2026-03-24T03:12:56.094606Z DEBUG devices::virtio::mmio[vsock]] set_irq_line: 7
```

---

## 阶段 10：FDT 创建与系统配置

### #14  Vmm::configure_system
```
文件：src/vmm/src/vmm.rs
行号：269
```

**作用**: 配置系统硬件并创建 FDT

**调用栈**:
```
#14 Vmm::configure_system (
        &mut self,
        guest_mem: &GuestMemoryMmap,
        arch_memory_info: &ArchMemoryInfo,
        num_vcpu: u32,
        cmdline: &str,
        device_info: &HashMap,
        intc: &IrqChip,
        initrd: &Option<InitrdConfig>,
    ) -> Result<()>
    at src/vmm/src/vmm.rs:269
    #15 fdt::loongarch64::create_fdt()
    #16 arch::loongarch64::configure_system()
```

---

### #15  create_fdt
```
文件：src/devices/src/fdt/loongarch64.rs
行号：51
```

**作用**: 创建扁平设备树 (FDT)

**调用栈**:
```
#15 create_fdt (
        guest_mem: &GuestMemoryMmap,
        arch_memory_info: &ArchMemoryInfo,
        num_vcpu: u32,
        cmdline: &str,
        device_info: &HashMap,
        intc: &IrqChip,
        initrd: &Option<InitrdConfig>,
    ) -> Result<Vec<u8>>
    at src/devices/src/fdt/loongarch64.rs:51
```

**FDT 节点创建流程**:
```rust
pub fn create_fdt(...) -> Result<Vec<u8>> {
    let mut fdt = FdtWriter::new();
    
    // 1. CPU 节点
    create_cpu_nodes(&mut fdt, num_vcpu)?;
    
    // 2. 内存节点
    create_memory_node(&mut fdt, guest_mem, arch_memory_info)?;
    
    // 3. Chosen 节点（启动参数）
    create_chosen_node(&mut fdt, cmdline, initrd, device_info)?;
    
    // 4. 中断控制器节点
    create_cpuintc_node(&mut fdt)?;        // CPU INTC
    create_eiointc_node(&mut fdt)?;        // EIOINTC
    create_pic_node(&mut fdt, intc)?;      // PCHPIC
    
    // 5. 设备节点
    create_devices_node(&mut fdt, device_info, intc)?;
        ├── create_serial_node()
        └── create_virtio_node()
    
    // 6. 写入 FDT 到 guest 内存
    let fdt_final = fdt.finish()?;
    guest_mem.write_slice(&fdt_final, fdt_addr)?;
    
    debug!("loongarch fdt written: addr=0x{:x}, size=0x{:x}",
           fdt_addr, fdt_final.len());
    
    Ok(fdt_final)
}
```

**日志输出**:
```
[2026-03-24T03:12:56.094638Z DEBUG devices::fdt::loongarch64] 
    loongarch chosen: has_serial=true, has_virtio_console=true, stdout_path=None
[2026-03-24T03:12:56.094665Z DEBUG devices::fdt::loongarch64] 
    loongarch serial node: addr=0xa001000, len=0x1000, irq=2, clock-frequency=3686400
[2026-03-24T03:12:56.094671Z DEBUG devices::fdt::loongarch64] 
    loongarch virtio node: addr=0xa002000, irq=3
[2026-03-24T03:12:56.094676Z DEBUG devices::fdt::loongarch64] 
    loongarch virtio node: addr=0xa003000, irq=4
[2026-03-24T03:12:56.094681Z DEBUG devices::fdt::loongarch64] 
    loongarch virtio node: addr=0xa004000, irq=5
[2026-03-24T03:12:56.094686Z DEBUG devices::fdt::loongarch64] 
    loongarch virtio node: addr=0xa005000, irq=6
[2026-03-24T03:12:56.094690Z DEBUG devices::fdt::loongarch64] 
    loongarch virtio node: addr=0xa006000, irq=7
[2026-03-24T03:12:56.094698Z DEBUG devices::fdt::loongarch64] 
    loongarch fdt written: addr=0xbfff0000, size=0x73f
```

---

### #16  setup_fdt_system_table
```
文件：src/arch/src/loongarch64/linux/efi.rs
行号：85
```

**作用**: 创建 EFI System Table 并指向 FDT

**调用栈**:
```
#16 setup_fdt_system_table (
        mem: &GuestMemoryMmap,
        info: &ArchMemoryInfo,
    ) -> Result<()>
    at src/arch/src/loongarch64/linux/efi.rs:85
```

**关键代码**:
```rust
pub fn setup_fdt_system_table(
    mem: &GuestMemoryMmap,
    info: &ArchMemoryInfo,
) -> Result<()> {
    let systab_addr = GuestAddress(info.efi_system_table_addr);
    let config_addr = systab_addr.unchecked_add(EFI_CONFIG_TABLE_OFFSET);
    let vendor_addr = systab_addr.unchecked_add(EFI_VENDOR_OFFSET);

    // 1. 创建 EFI Config Table（包含 FDT 地址）
    let config = EfiConfigTable64 {
        guid: DEVICE_TREE_GUID,
        table: info.fdt_addr,
    };
    mem.write_obj(config, config_addr)?;

    // 2. 创建 EFI System Table
    let systab = EfiSystemTable64 {
        hdr: EfiTableHeader {
            signature: EFI_SYSTEM_TABLE_SIGNATURE,
            revision: EFI_2_10_SYSTEM_TABLE_REVISION,
            headersize: size_of::<EfiSystemTable64>() as u32,
            crc32: 0,
            reserved: 0,
        },
        fw_vendor: vendor_addr.raw_value(),
        fw_revision: 0,
        nr_tables: 1,
        tables: config_addr.raw_value(),
        ..Default::default()
    };
    mem.write_obj(systab, systab_addr)?;

    // 3. 写入 vendor 字符串 "libkrun"
    let vendor: [u16; 8] = [
        b'l', b'i', b'b', b'k', b'r', b'u', b'n', 0,
    ];
    for (i, ch) in vendor.iter().enumerate() {
        mem.write_obj(*ch, vendor_addr.unchecked_add((i*2) as u64))?;
    }

    debug!("loongarch efi handoff: systab=0x{:x}, config=0x{:x}, vendor=0x{:x}, fdt=0x{:x}",
           systab_addr.raw_value(), config_addr.raw_value(),
           vendor_addr.raw_value(), info.fdt_addr);

    Ok(())
}
```

**日志输出**:
```
[2026-03-24T03:12:56.094704Z DEBUG arch::loongarch64::linux::efi] 
    loongarch efi handoff: systab=0xbffec000, config=0xbffec100, 
    vendor=0xbffec200, fdt=0xbfff0000
```

---

## 阶段 11：vCPU 启动

### #17  Vmm::start_vcpus
```
文件：src/vmm/src/vmm.rs
行号：223
```

**作用**: 启动所有 vCPU 线程

**调用栈**:
```
#17 Vmm::start_vcpus (&mut self) -> Result<()>
    at src/vmm/src/vmm.rs:223
    #18 Vcpu::register_kick_signal_handler()
    #19 Vcpu::start_threaded()
```

**关键代码**:
```rust
pub fn start_vcpus(&mut self) -> Result<()> {
    for vcpu in &mut self.vcpus {
        // 1. 注册唤醒信号处理器
        vcpu.register_kick_signal_handler();
        
        // 2. 启动 vCPU 线程
        vcpu.start_threaded()?;
    }
    
    // 3. 发送 Resume 事件
    for vcpu in &mut self.vcpus {
        vcpu.resume()?;
    }
    
    Ok(())
}
```

---

### #18  Vcpu::run
```
文件：src/vmm/src/linux/vstate.rs
行号：1654
```

**作用**: vCPU 主循环

**调用栈**:
```
#18 Vcpu::run (&mut self)
    at src/vmm/src/linux/vstate.rs:1654
    #19 StateMachine::run(self, Self::paused)
    #20 Vcpu::running()  // running 状态
    #21 Vcpu::run_emulation()
    #22 self.fd.run()  // KVM_RUN ioctl
```

**关键代码**:
```rust
pub fn run(&mut self) {
    StateMachine::run(self, Self::paused);
}

fn running(&mut self) -> StateMachine<Self> {
    loop {
        match self.run_emulation() {
            Ok(VcpuEmulation::Handled) => continue,
            Ok(VcpuEmulation::Interrupted) => break,
            Ok(VcpuEmulation::Stopped) => return self.exit(FC_EXIT_CODE_OK),
            Err(_) => return self.exit(FC_EXIT_CODE_GENERIC_ERROR),
        }
    }
    // 检查外部事件...
}

fn run_emulation(&mut self) -> Result<VcpuEmulation> {
    match self.fd.run() {  // KVM_RUN ioctl
        Ok(exit_reason) => match exit_reason {
            VcpuExit::MmioRead(addr, data) => {
                self.mmio_bus.read(addr, data);
                Ok(VcpuEmulation::Handled)
            }
            VcpuExit::MmioWrite(addr, data) => {
                self.mmio_bus.write(addr, data);
                Ok(VcpuEmulation::Handled)
            }
            VcpuExit::IocsrRead(addr, data) => {
                self.handle_iocsr_read(addr, data)?;
                Ok(VcpuEmulation::Handled)
            }
            VcpuExit::IocsrWrite(addr, data) => {
                self.handle_iocsr_write(addr, data)?;
                Ok(VcpuEmulation::Handled)
            }
            VcpuExit::Hlt | VcpuExit::Shutdown => {
                Ok(VcpuEmulation::Stopped)
            }
            _ => Err(Error::VcpuUnhandledKvmExit),
        },
        Err(e) => match e.errno() {
            libc::EAGAIN => Ok(VcpuEmulation::Handled),
            libc::EINTR => Ok(VcpuEmulation::Interrupted),
            _ => Err(Error::VcpuUnhandledKvmExit),
        },
    }
}
```

---

## 核心概念与架构详解

本节详细解释 libkrun/KVM 虚拟化架构中的核心概念、组件和运行机制。

### 虚拟化架构总览

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                           用户空间 (Userspace)                                │
│                           ─────────────                                       │
│                                                                             │
│  ┌─────────────────────────────────────────────────────────────────────┐   │
│  │                    libkrun (VMM - Virtual Machine Manager)          │   │
│  │                                                                     │   │
│  │  ┌───────────────┐  ┌───────────────┐  ┌───────────────┐           │   │
│  │  │  Balloon 设备 │  │   RNG 设备    │  │  Console 设备 │  ...      │   │
│  │  │  (内存气球)   │  │ (随机数生成器)│  │  (虚拟控制台) │           │   │
│  │  └───────────────┘  └───────────────┘  └───────────────┘           │   │
│  │                                                                     │   │
│  │  ┌───────────────────────────────────────────────────────────────┐ │   │
│  │  │                    Virtio 设备模型                             │ │   │
│  │  │  ┌─────────┐ ┌─────────┐ ┌─────────┐ ┌─────────┐ ┌─────────┐ │ │   │
│  │  │  │ Balloon │ │   RNG   │ │ Console │ │   FS    │ │  Block  │ │ │   │
│  │  │  └─────────┘ └─────────┘ └─────────┘ └─────────┘ └─────────┘ │ │   │
│  │  └───────────────────────────────────────────────────────────────┘ │   │
│  │                                                                     │   │
│  │  ┌───────────────────────────────────────────────────────────────┐ │   │
│  │  │                    Legacy 设备模型                             │ │   │
│  │  │  ┌─────────────────┐  ┌─────────────────┐                     │ │   │
│  │  │  │   Serial (串口) │  │  RTC (实时时钟) │                     │ │   │
│  │  │  └─────────────────┘  └─────────────────┘                     │ │   │
│  │  └───────────────────────────────────────────────────────────────┘ │   │
│  │                                                                     │   │
│  │  ┌───────────────────────────────────────────────────────────────┐ │   │
│  │  │                  MMIO 设备管理器                               │ │   │
│  │  │  将 Virtio/Legacy 设备映射到 MMIO 总线                          │ │   │
│  │  └───────────────────────────────────────────────────────────────┘ │   │
│  └─────────────────────────────────────────────────────────────────────┘   │
│                                                                             │
│  ┌─────────────────────────────────────────────────────────────────────┐   │
│  │                    KVM 内核模块 (Kernel Module)                     │   │
│  │                                                                     │   │
│  │  ┌───────────────────────────────────────────────────────────────┐ │   │
│  │  │  /dev/kvm (KVM 字符设备)                                       │ │   │
│  │  │  ├── VM fd (ioctl: KVM_CREATE_VM)                             │ │   │
│  │  │  │   └── VCPU fd (ioctl: KVM_CREATE_VCPU) × N                 │ │   │
│  │  │  └── 中断控制器设备                                            │ │   │
│  │  │      ├── IPI fd                                               │ │   │
│  │  │      ├── EIOINTC fd                                           │ │   │
│  │  │      └── PCHPIC fd                                            │ │   │
│  │  └───────────────────────────────────────────────────────────────┘ │   │
│  └─────────────────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────────────────┘
                              ↕ ioctl / mmap ↕
┌─────────────────────────────────────────────────────────────────────────────┐
│                        内核空间 (Kernel Space)                               │
│                        ───────────────                                       │
│                                                                             │
│  ┌─────────────────────────────────────────────────────────────────────┐   │
│  │                    KVM 主模块 (kvm.ko)                               │   │
│  │  - VM 生命周期管理                                                   │   │
│  │  - 内存虚拟化                                                        │   │
│  │  - 中断虚拟化                                                        │   │
│  └─────────────────────────────────────────────────────────────────────┘   │
│                                                                             │
│  ┌─────────────────────────────────────────────────────────────────────┐   │
│  │                    KVM 架构模块 (kvm-loongarch.ko)                   │   │
│  │  - LoongArch 特定的虚拟化扩展                                        │   │
│  │  - VM Entry/Exit 处理                                                │   │
│  │  - 寄存器虚拟化                                                      │   │
│  └─────────────────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────────────────┘
                              ↕ VM Entry/Exit ↕
┌─────────────────────────────────────────────────────────────────────────────┐
│                          Guest 虚拟机                                        │
│                          ────────────                                        │
│                                                                             │
│  ┌─────────────────────────────────────────────────────────────────────┐   │
│  │                    Guest OS (Linux)                                 │   │
│  │                                                                     │   │
│  │  ┌───────────────────────────────────────────────────────────────┐ │   │
│  │  │  Guest Kernel (非特权模式 - non-root)                          │ │   │
│  │  │  ├── Virtio 驱动 (virtio_mmio.ko)                             │ │   │
│  │  │  ├── 串口驱动 (8250_uart.ko)                                  │ │   │
│  │  │  └── 其他设备驱动                                             │ │   │
│  │  └───────────────────────────────────────────────────────────────┘ │   │
│  └─────────────────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────────────────┘
```

---

### VMM / MicroVM / VM / vCPU 概念辨析

#### VMM (Virtual Machine Manager)

**定义**: 虚拟机管理器，是创建和管理虚拟机的软件层

**在 libkrun 中**:
- `Vmm` 结构体是核心管理结构
- 负责管理所有 vCPU、设备、内存
- 运行在**用户空间**

```rust
// src/vmm/src/vmm.rs
pub struct Vmm {
    guest_memory: GuestMemoryMmap,      // Guest 物理内存映射
    arch_memory_info: ArchMemoryInfo,   // 架构特定内存信息
    kernel_cmdline: Cmdline,            // 内核命令行
    vcpus_handles: Vec<VcpuHandle>,     // vCPU 线程句柄
    exit_evt: EventFd,                  // VM 退出事件
    exit_code: Arc<AtomicI32>,          // VM 退出代码
    vm: Vm,                             // KVM VM 文件描述符
    mmio_device_manager: MMIODeviceManager,  // MMIO 设备管理器
}
```

**VMM 的职责**:
1. 创建和初始化 VM（通过 KVM ioctl）
2. 创建和配置 vCPU
3. 添加和配置虚拟设备
4. 启动 vCPU 线程
5. 处理 VM 退出事件
6. 管理设备模拟

#### MicroVM

**定义**: 一种轻量级虚拟机架构，专为容器和 serverless 场景设计

**特点**:
- 极简的设备模型（只有必要的设备）
- 快速启动（<125ms）
- 小内存占用
- 使用 Virtio 设备

**libkrun 中的 MicroVM**:
- `build_microvm()` 函数创建 MicroVM
- 默认设备：balloon、rng、console、fs
- 可选设备：block、net、gpu、vsock

#### VM (Virtual Machine)

**定义**: 虚拟机实例，包含 Guest 内存、虚拟硬件配置

**两层含义**:

1. **KVM VM** (内核空间):
   - 通过 `KVM_CREATE_VM` ioctl 创建
   - 返回 VM 文件描述符
   - 管理 Guest 物理内存映射
   - 创建 vCPU

2. **逻辑 VM** (用户空间):
   - VMM 管理的完整虚拟机实例
   - 包括内存、设备、vCPU
   - 对应一个 Guest OS 实例

#### vCPU (Virtual CPU)

**定义**: 虚拟 CPU，Guest OS 看到的"CPU"

**实现方式**:
- 每个 vCPU 是一个**用户空间线程**
- 线程内循环执行 `KVM_RUN` ioctl
- `KVM_RUN` 让 CPU 进入 **non-root 模式** 执行 Guest 代码

```rust
// src/vmm/src/linux/vstate.rs
pub struct Vcpu {
    fd: VcpuFd,                    // KVM vCPU 文件描述符
    id: u8,                        // vCPU ID (0, 1, 2...)
    mmio_bus: Option<Bus>,         // MMIO 总线（处理设备 MMIO 访问）
    exit_evt: EventFd,             // vCPU 退出事件
    // ... 其他字段
}
```

**vCPU 运行模式**:
```
用户空间线程
    ↓
KVM_RUN ioctl  ──────────────┐
    ↓                         │
┌─────────────────────────────┤
│ KVM 内核模块                 │
│    ↓                        │
│ VM Entry (进入 non-root)    │  ← Root 模式 (完整特权)
│    ↓                        │
│ Guest 代码执行 (non-root)   │  ← Non-root 模式 (受限特权)
│    ↓                        │
│ VM Exit (退出到 root)       │
└─────────────────────────────┤
    ↓                         │
KVM_RUN 返回                  │
    ↓                         │
用户空间处理 VM Exit ─────────┘
```

---

### KVM 运行模式详解

#### Root Mode vs Non-root Mode

现代 CPU 的虚拟化扩展（如 Intel VT-x、AMD-V、LoongArch-V）引入了两种操作模式：

```
┌──────────────────────────────────────────────────────────────────┐
│                    CPU 特权级别                                   │
│                                                                  │
│  Root Mode (根模式)          Non-root Mode (非根模式)            │
│  ─────────────────           ────────────────────                │
│  • KVM 内核模块运行在这里     • Guest OS 运行在这里                │
│  • 完整特权级别               • 受限的特权级别                     │
│  • 可以执行所有指令           • 某些指令会触发 VM Exit            │
│  • 可以访问所有资源           • 访问特定资源会触发 VM Exit        │
│                                                                  │
│  例如：                    例如：                                │
│  - KVM 模块初始化             - Guest 执行普通用户代码             │
│  - 处理 VM Exit              - Guest 执行内核代码                 │
│  - 配置虚拟硬件              - Guest 访问 MMIO → VM Exit         │
│                            - Guest 执行特权指令 → VM Exit       │
└──────────────────────────────────────────────────────────────────┘
```

#### VM Entry 和 VM Exit

**VM Entry**:
- KVM 通过 `VMLAUNCH`/`VMRESUME` (x86) 或专用指令 (LoongArch) 进入
- CPU 从 root mode 切换到 non-root mode
- 开始执行 Guest 代码（从 vCPU 的 PC 寄存器指向的地址）

**VM Exit** (退出原因):
1. **特权指令**: Guest 执行了只能在 root mode 执行的指令
2. **MMIO 访问**: Guest 访问了映射到 MMIO 的地址
3. **IOCSR 访问**: LoongArch 特有的 CSR 寄存器访问
4. **中断**: 外部中断到达
5. **HLT**: Guest 执行了停机指令
6. **错误**: 虚拟化处理失败

**VM Exit 处理流程**:
```
Guest 执行
    ↓
访问 MMIO 地址 (例如 0xa002000)
    ↓
VM Exit (自动，硬件行为)
    ↓
KVM 内核捕获退出
    ↓
KVM_RUN 返回到用户空间
    ↓
VMM 检查退出原因 (mmio_exit)
    ↓
VMM 查找 MMIO 设备 (virtio-fs)
    ↓
VMM 模拟设备行为 (处理读写)
    ↓
KVM_RUN 再次调用
    ↓
VM Entry (返回 Guest)
    ↓
Guest 继续执行 (仿佛 MMIO 操作已完成)
```

---

### 设备模拟：User Space vs Kernel Space

#### 哪些设备在 User Space 模拟？

**所有 Virtio 设备**:
- Balloon (内存气球)
- RNG (随机数生成器)
- Console (虚拟控制台)
- FS (文件系统 - VirtioFS)
- Block (块设备)
- Net (网络设备)
- GPU (图形设备)
- Vsock (VM 间通信)

**Legacy 设备**:
- Serial (串口) - MMIO 方式
- RTC (实时时钟) - aarch64 专用

**为什么在 User Space？**:
1. **灵活性**: 易于实现复杂的设备逻辑
2. **安全性**: 设备代码崩溃不影响内核
3. **可维护性**: Rust 编写，类型安全
4. **与 KVM 解耦**: 不依赖内核模块更新

#### 哪些在中核空间处理？

**KVM 内核模块**:
- VM/vCPU 生命周期管理
- 内存虚拟化 (EPT/NPT)
- 中断虚拟化 (APICv)
- 基本的寄存器虚拟化

**设备驱动 (Guest 侧)**:
- Virtio 驱动 (`virtio_mmio.ko`)
- 串口驱动 (`8250_uart.ko`)
- 其他 Guest OS 驱动

---

### MMIO 设备映射总览

**MMIO** = **Memory-Mapped I/O**（内存映射 I/O）

所有虚拟设备（包括中断控制器）都通过 MMIO 映射到 Guest 物理内存地址空间。

#### LoongArch MMIO 地址布局

```
Guest 物理内存布局 (LoongArch64)

0x0000_0000_0000_0000
├─────────────────────┐ ← 0x0000_0000
│     Firmware        │ FIRMWARE_START (0)
│   (如果有 EFI)      │
├─────────────────────┤
│                     │
│       保留区        │
│                     │
├─────────────────────┤ ← 0x0A00_0000 (MAPPED_IO_START)
│    MMIO 设备区       │  所有虚拟设备映射在这里
│                     │
│  ┌───────────────┐  │
│  │ 串口 0        │  │ 0x0a001000 (ns16550a UART)
│  ├───────────────┤  │
│  │ PCHPIC        │  │ 0x10000000 ← 中断控制器
│  ├───────────────┤  │
│  │ Virtio Balloon│  │ 0x0a002000
│  ├───────────────┤  │
│  │ Virtio Rng    │  │ 0x0a003000
│  ├───────────────┤  │
│  │ Virtio Console│  │ 0x0a004000
│  ├───────────────┤  │
│  │ Virtio FS     │  │ 0x0a005000
│  ├───────────────┤  │
│  │ Virtio Vsock  │  │ 0x0a006000
│  └───────────────┘  │
├─────────────────────┤ ← 0x4000_0000 (DRAM_MEM_START)
│                     │
│       Kernel        │  内核镜像加载地址
│                     │
├─────────────────────┤
│        RAM          │  主内存区域
│                     │
└─────────────────────┘
```

#### MMIO 设备寄存器表

| 设备 | MMIO 基地址 | 大小 | 说明 |
|------|------------|------|------|
| Serial (UART) | `0x0a001000` | `0x1000` | ns16550a 兼容串口 |
| PCHPIC | `0x10000000` | `0x400` | PCI 主机中断控制器 |
| Virtio Balloon | `0x0a002000` | `0x200` | 内存气球设备 |
| Virtio Rng | `0x0a003000` | `0x200` | 随机数生成器 |
| Virtio Console | `0x0a004000` | `0x200` | 虚拟控制台 |
| Virtio FS | `0x0a005000` | `0x200` | 文件系统设备 |
| Virtio Vsock | `0x0a006000` | `0x200` | VM 间通信设备 |

**注意**：每个 Virtio 设备占用 `0x200` (512 字节) 的 MMIO 空间，包含 Virtio MMIO 寄存器。

---

### EventFd 类型与用途

**EventFd** 是 Linux 内核提供的轻量级事件通知机制

```c
// 基本用法
int efd = eventfd(0, EFD_NONBLOCK);  // 创建
write(efd, &value, sizeof(value));   // 通知（加计数）
read(efd, &value, sizeof(value));    // 等待并清除
```

#### libkrun 中的 EventFd 类型

| EventFd 名称 | 位置 | 用途 | 触发者 | 接收者 |
|-------------|------|------|--------|--------|
| `interrupt_evt` | Virtio 设备 | 设备中断通知 | 设备 worker 线程 | irqchip |
| `exit_evt` | VMM/vCPU | VM 退出通知 | vCPU 线程 | VMM 主线程 |
| `kick_evt` | vCPU | vCPU 唤醒通知 | VMM 主线程 | vCPU 线程 |
| `irq_sync` | MMIO 设备 | 中断同步 | - | - |

---

### 中断控制器详解：intc / IrqChip / KvmLoongArchIrqChip

#### intc 是什么？

**intc** = **Interrupt Controller**（中断控制器）

在 libkrun 代码中，`intc` 是 `IrqChip` 类型的别名/缩写，是一个**架构相关的中断控制器抽象**。

#### IrqChip 类型定义

```rust
// src/devices/src/legacy/irqchip.rs
pub type IrqChip = Arc<Mutex<IrqChipDevice>>;

pub struct IrqChipDevice {
    inner: Box<dyn IrqChipT>,  // 具体的中断控制器实现
}
```

**IrqChip 是一个智能指针包装器**，内部包含一个实现了 `IrqChipT` trait 的具体设备。

#### 不同架构的 IrqChipT 实现

| 架构 | 具体实现 | 对应硬件 |
|------|----------|----------|
| **x86_64** | `KvmIoapic` | IOAPIC (I/O Advanced PIC) |
| **aarch64** | `KvmGic` | GIC (Generic Interrupt Controller) |
| **riscv64** | `KvmAia` | AIA (Advanced Interrupt Architecture) |
| **loongarch64** | `KvmLoongArchIrqChip` | IPI + EIOINTC + PCHPIC |

#### LoongArch 的 KvmLoongArchIrqChip

**KvmLoongArchIrqChip 包含三个 KVM 设备**：

```rust
// src/devices/src/legacy/kvmloongarchirqchip.rs
pub struct KvmLoongArchIrqChip {
    _ipi_fd: DeviceFd,         // IPI 设备文件描述符
    _eiointc_fd: DeviceFd,     // EIOINTC 设备文件描述符
    _pchpic_fd: DeviceFd,      // PCHPIC 设备文件描述符
    irq_vcpu_fd: File,         // 用于 KVM_INTERRUPT 的 vCPU fd
    vcpu_count: u32,           // vCPU 数量
}
```

**这三个设备的关系**：

```
┌─────────────────────────────────────────────────────────────────┐
│                    LoongArch 中断控制器层级                       │
│                                                                 │
│  ┌─────────────────────────────────────────────────────────┐   │
│  │  KvmLoongArchIrqChip (用户空间抽象)                      │   │
│  │                                                          │   │
│  │  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐  │   │
│  │  │  IPI Device  │  │ EIOINTC Dev  │  │ PCHPIC Dev   │  │   │
│  │  │  (KVM fd)    │  │  (KVM fd)    │  │  (KVM fd)    │  │   │
│  │  │              │  │              │  │              │  │   │
│  │  │ KVM_DEV_    │  │ KVM_DEV_    │  │ KVM_DEV_    │  │   │
│  │  │ LOONGARCH_  │  │ LOONGARCH_  │  │ LOONGARCH_  │  │   │
│  │  │ IPI         │  │ EIOINTC     │  │ PCHPIC      │  │   │
│  │  └──────────────┘  └──────────────┘  └──────────────┘  │   │
│  └─────────────────────────────────────────────────────────┘   │
│                                                                 │
│  注意：当前实现主要使用 KVM_INTERRUPT 通过 cpuintc 注入中断       │
│       这三个设备主要是为了平台兼容性保留                         │
└─────────────────────────────────────────────────────────────────┘
```

#### 三个中断控制器的职责与 MMIO 映射

| 设备 | 全称 | MMIO 地址 | 大小 | 职责 |
|------|------|----------|------|------|
| **IPI** | Inter-Processor Interrupt | 内核管理 | - | 处理器间中断（多核通信） |
| **EIOINTC** | Extended I/O Interrupt Controller | 内核管理 | - | 扩展 I/O 中断控制器（路由） |
| **PCHPIC** | PCI Host PIC | `0x1000_0000` | `0x400` | PCI 主机中断控制器（传统 PIC 模拟） |

**注意**：IPI 和 EIOINTC 的 MMIO 地址由 KVM 内核模块管理，用户空间不需要直接访问。PCHPIC 的 MMIO 地址是 `0x1000_0000`，大小为 `0x400` 字节（1KB）。

#### 中断控制器的 MMIO 映射详解

**重要概念**：中断控制器作为设备，也是通过**MMIO 映射**到 Guest 物理内存的。Guest 内核通过访问 MMIO 地址来读写中断控制器寄存器。

**PCHPIC MMIO 实现**：

```rust
// src/devices/src/legacy/kvmloongarchirqchip.rs
impl IrqChipT for KvmLoongArchIrqChip {
    /// 返回 PCHPIC 的 MMIO 基地址
    fn get_mmio_addr(&self) -> u64 {
        0x1000_0000  // PCHPIC 的 MMIO 基地址
    }

    /// 返回 PCHPIC 的 MMIO 区域大小
    fn get_mmio_size(&self) -> u64 {
        0x400  // 1KB 大小
    }
}
```

**Guest 内存布局中的位置**：

```
Guest 物理内存布局 (MMIO 区域)

0x0A00_0000 (MAPPED_IO_START)
├─────────────────────┐
│    MMIO 设备区       │
│                     │
│  ┌───────────────┐  │ 0x0a001000
│  │ 串口 0        │  │ (ns16550a UART, 4KB)
│  ├───────────────┤  │
│  │               │  │
│  ├───────────────┤  │ 0x10000000
│  │ PCHPIC        │  │ ← 中断控制器 MMIO (1KB)
│  ├───────────────┤  │
│  │               │  │
│  ├───────────────┤  │ 0x0a002000
│  │ Virtio MMIO   │  │ ← Virtio 设备 (每个 512 字节)
│  └───────────────┘  │
├─────────────────────┤ ← 0x4000_0000 (DRAM_MEM_START)
│                     │
│    Kernel Image     │
│                     │
└─────────────────────┘
```

**FDT 中的描述**：

```rust
// src/devices/src/fdt/loongarch64.rs
fn create_pic_node(fdt: &mut FdtWriter, intc: &IrqChip) -> Result<()> {
    // 读取 PCHPIC 的 MMIO 地址和大小
    let reg = [
        0x0_u64,                    // 地址高 32 位
        intc.get_mmio_addr(),       // 地址低 32 位 = 0x10000000
        0x0_u64,                    // 大小高 32 位
        intc.get_mmio_size(),       // 大小低 32 位 = 0x400
    ];

    // 创建设备树节点
    let node = fdt.begin_node(&format!("interrupt-controller@{:x}", intc.get_mmio_addr()))?;
    fdt.property_string("compatible", "loongson,pch-pic-1.0")?;
    fdt.property_array_u64("reg", &reg)?;  // 注册 MMIO 区域
    fdt.property_null("interrupt-controller")?;
    fdt.property_u32("#interrupt-cells", 2)?;
    fdt.property_u32("phandle", PCH_PIC_PHANDLE)?;
    fdt.property_u32("interrupt-parent", EIOINTC_PHANDLE)?;
    fdt.end_node(node)?;
    Ok(())
}
```

**FDT 输出示例**：

```dts
interrupt-controller@10000000 {
    compatible = "loongson,pch-pic-1.0";
    reg = <0x0 0x10000000 0x0 0x400>;  // MMIO 基地址 0x10000000, 大小 1KB
    interrupt-controller;
    #interrupt-cells = <2>;
    phandle = <3>;
    interrupt-parent = <2>;  // 父节点是 EIOINTC
};
```

**Guest 内核如何访问 PCHPIC**：

```c
// Guest 内核驱动（drivers/irqchip/irq-loongarch-pchpic.c）

// 1. 定义 MMIO 资源
static struct resource pchpic_resources[] = {
    {
        .start  = 0x10000000,  // MMIO 基地址
        .end    = 0x100003ff,  // 基地址 + 大小 - 1 = 0x10000000 + 0x400 - 1
        .flags  = IORESOURCE_MEM,
    },
};

// 2. 初始化函数
void __init pchpic_init(struct device_node *node)
{
    // 从设备树解析 MMIO 区域
    struct resource res;
    of_address_to_resource(node, 0, &res);
    // res.start = 0x10000000
    
    // 映射到内核虚拟地址
    pchpic_base = ioremap(res.start, resource_size(&res));
    //                ↑ 现在可以通过 pchpic_base 访问 PCHPIC 寄存器
    
    // 读写 PCHPIC 寄存器
    writel(0x1, pchpic_base + PCHPIC_EN);      // 使能中断
    u32 val = readl(pchpic_base + PCHPIC_STATUS);  // 读取状态
}
```

**MMIO 访问完整流程**：

```
┌─────────────────────────────────────────────────────────────────┐
│  Guest 内核执行                                                  │
│                                                                 │
│  pchpic_write_reg(reg=PCHPIC_EN, value=0x1)                     │
│      ↓                                                          │
│  writel(0x1, pchpic_base + 0xXX)                               │
│      ↓                                                          │
│  CPU 访问 Guest 物理地址 0x100000XX                              │
└─────────────────────────────────────────────────────────────────┘
                              ↓
                    MMIO 访问检测
                    (地址不在 RAM 范围内)
                              ↓
┌─────────────────────────────────────────────────────────────────┐
│  KVM 内核空间                                                    │
│                                                                 │
│  VM Exit (退出原因：KVM_EXIT_MMIO)                              │
│      ↓                                                          │
│  KVM 填充 mmio 结构：                                            │
│    - mmio.addr = 0x100000XX                                     │
│    - mmio.data = 0x1                                            │
│    - mmio.len = 4                                               │
│    - mmio.is_write = 1                                          │
│      ↓                                                          │
│  KVM_RUN 返回到用户空间                                          │
└─────────────────────────────────────────────────────────────────┘
                              ↓
                    KVM_RUN ioctl 返回
                              ↓
┌─────────────────────────────────────────────────────────────────┐
│  User Space (libkrun VMM)                                       │
│                                                                 │
│  检查退出原因：KVM_EXIT_MMIO                                     │
│      ↓                                                          │
│  VMM 查找 MMIO 设备：mmio_bus.find(0x100000XX)                   │
│      ↓                                                          │
│  找到 PCHPIC 设备                                                 │
│      ↓                                                          │
│  调用 PCHPIC 的 write() 方法：                                    │
│    KvmLoongArchIrqChip::write(                                  │
│      vcpuid=0,                                                   │
│      offset=0xXX,                                                │
│      data=[0x1, 0x0, 0x0, 0x0]                                  │
│    )                                                            │
│      ↓                                                          │
│  更新 PCHPIC 内部状态（如使能某条中断线）                          │
│      ↓                                                          │
│  VMM 准备再次进入 Guest                                           │
│      ↓                                                          │
│  调用 KVM_RUN                                                    │
└─────────────────────────────────────────────────────────────────┘
                              ↓
                    VM Entry
                              ↓
┌─────────────────────────────────────────────────────────────────┐
│  Guest 内核                                                     │
│                                                                 │
│  继续执行（仿佛写操作已完成）                                     │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

#### 为什么当前实现不通过 PCHPIC MMIO 注入中断？

虽然 PCHPIC 有 MMIO 映射，但当前 libkrun 的 LoongArch 实现**主要使用 `KVM_INTERRUPT` 直接注入到 CPU INTC**，原因如下：

1. **历史原因**：旧版 LoongArch KVM 的外部中断控制器链路（irqfd/routing）不完整
2. **简化实现**：`KVM_INTERRUPT` 更直接，不需要模拟完整的中断路由
3. **兼容性**：IPI/EIOINTC/PCHPIC 设备保留用于未来可能的平台兼容性需求

**当前中断路径**（实际使用）：
```
Virtio 设备完成 I/O
    ↓
signal_used_queue()
    ↓
KVM_INTERRUPT ioctl (signed_irq=+6)
    ↓
KVM 内核 → kvm_queue_irq(vcpu, 6)
    ↓
设置 CPU pending HWI6
    ↓
Guest CPU 收到中断
```

**传统中断路径**（未使用）：
```
Virtio 设备完成 I/O
    ↓
signal_used_queue()
    ↓
写入 eventfd
    ↓
KVM irqfd 机制
    ↓
PCHPIC MMIO 寄存器更新 (Guest 访问 0x10000000)
    ↓
EIOINTC 路由
    ↓
CPU INTC
    ↓
Guest CPU 收到中断
```

**PCHPIC MMIO 的用途**：
- 虽然当前中断注入不使用 PCHPIC MMIO 路径
- 但 PCHPIC 的 MMIO 映射仍然存在于 FDT 中
- Guest 内核可以读取/写入 PCHPIC 寄存器（用于兼容性）
- 未来可能启用完整的中断路由链路

#### 当前实现的中断注入方式

**重要**：当前 libkrun 的 LoongArch 实现**主要使用 `KVM_INTERRUPT` 直接注入到 CPU INTC**，而不是通过上述三个设备。

```rust
// src/devices/src/legacy/kvmloongarchirqchip.rs
impl IrqChipT for KvmLoongArchIrqChip {
    fn set_irq_state(
        &self,
        irq_line: Option<u32>,
        _interrupt_evt: Option<&EventFd>,
        active: bool,
    ) -> Result<(), DeviceError> {
        let irq = match irq_line {
            Some(irq) => irq,
            None => return Err(...),
        };

        // 计算有符号中断号
        let signed_irq = if active {
            irq as i32       // 正数 = 断言
        } else {
            -(irq as i32)    // 负数 = 撤销
        };

        // 使用 KVM_INTERRUPT 直接注入到 vCPU
        let interrupt = kvm_interrupt {
            irq: signed_irq as u32,
        };
        let ret = unsafe {
            ioctl_with_ref(&self.irq_vcpu_fd, KVM_INTERRUPT_LOONGARCH(), &interrupt)
        };
        
        // ...
    }
}
```

**为什么这样设计？**（代码注释中的解释）：

```rust
// Keep the in-kernel external irqchip devices around for platform
// compatibility; the active serial/virtio injection path uses
// KVM_INTERRUPT through cpuintc on vcpu0.
//
// 保留内核外部中断控制器设备以兼容平台；
// 但实际的串口/virtio 中断注入路径使用 KVM_INTERRUPT 通过 cpuintc。
```

**原因**：
1. 旧版 LoongArch KVM 的外部中断控制器链路（irqfd/routing）不完整
2. 使用 `KVM_INTERRUPT` + `cpuintc` 更直接、更可靠
3. IPI/EIOINTC/PCHPIC 设备保留用于未来可能的平台兼容性需求

#### intc 在代码中的使用

```rust
// src/vmm/src/builder.rs
// 创建中断控制器
intc = Arc::new(Mutex::new(IrqChipDevice::new(Box::new(
    KvmLoongArchIrqChip::new(
        vm.fd(),
        vm_resources.vm_config().vcpu_count.unwrap() as u32,
    )
    .unwrap(),
))));

// 附加设备时传递 intc
attach_legacy_devices(
    &vm,
    &mut mmio_device_manager,
    &mut kernel_cmdline,
    intc.clone(),  // ← 传递给串口等设备
    serial_devices,
)?;

// Virtio 设备也使用 intc
attach_balloon_device(&mut vmm, event_manager, intc.clone())?;
attach_rng_device(&mut vmm, event_manager, intc.clone())?;
attach_console_devices(&mut vmm, event_manager, intc.clone(), ...)?;
attach_fs_devices(&mut vmm, ..., intc.clone(), ...)?;
```

#### 中断注入的两种方式对比

| 方式 | 路径 | 优点 | 缺点 |
|------|------|------|------|
| **KVM_INTERRUPT** | VMM → KVM → CPU INTC | 简单直接，不依赖外部 irqchip | 不够"板级保真" |
| **irqfd/routing** | VMM → eventfd → KVM → PCHPIC → EIOINTC → CPU INTC | 更像真实硬件 | 旧内核支持不完整 |

**当前选择**：`KVM_INTERRUPT`（方式 1）

---

#### interrupt_evt 详解

```rust
// src/devices/src/virtio/mmio.rs
pub struct MmioTransport {
    status: AtomicUsize,        // 中断状态寄存器
    interrupt: Option<Interrupt>,
    // ...
}

struct Interrupt {
    irq_line: Option<u32>,      // 中断线号
    event: EventFd,             // interrupt_evt
    intc: IrqChip,              // 中断控制器
}

// 设备触发中断
fn signal_used_queue(&self) {
    let old = self.status.fetch_or(VIRTIO_MMIO_INT_VRING);
    if old == 0 {
        // 写入 eventfd 触发中断
        self.interrupt.event.write(1)?;
        //              ↑ 这就是 interrupt_evt
    }
}
```

**工作流程**:
```
VirtioFS Worker 处理完 I/O
    ↓
调用 signal_used_queue()
    ↓
写入 interrupt_evt (eventfd_write)
    ↓
eventfd 变为可读
    ↓
EventManager 检测到事件
    ↓
调用 irqchip.set_irq_state()
    ↓
KVM_INTERRUPT ioctl 注入中断到 Guest
```

#### exit_evt 详解

```rust
// src/vmm/src/linux/vstate.rs
pub struct Vcpu {
    exit_evt: EventFd,    // vCPU 退出时通知
    // ...
}

// vCPU 线程
fn running(&mut self) {
    loop {
        match self.run_emulation() {
            Ok(VcpuEmulation::Stopped) => {
                // VM 停止，通知主线程
                self.exit_evt.write(1)?;
                //         ↑ exit_evt
            }
        }
    }
}
```

**用途**: vCPU 线程退出时通知 VMM 主线程

---

### Virtio 设备详解

#### Balloon (内存气球)

**作用**: 动态调整 Guest 内存使用

**工作原理**:
```
Host 需要回收内存
    ↓
通过 Balloon 设备通知 Guest "inflate balloon"
    ↓
Guest 驱动分配内存页面（但不使用）
    ↓
Guest 将这些页面交给 Host
    ↓
Host 可以重用这些物理页面
```

**为什么需要？**:
- 内存超卖（overcommit）
- 动态内存管理
- 提高物理内存利用率

```rust
// src/devices/src/virtio/balloon/device.rs
pub struct Balloon {
    num_pages: AtomicU32,      // 当前气球大小（页面数）
    actual_pages: AtomicU32,   // 实际页面数
    // ...
}
```

#### RNG (随机数生成器)

**作用**: 为 Guest 提供真正的随机数

**为什么需要？**:
- VM 难以获取真正的熵（硬件事件）
- Guest 需要随机数用于加密密钥等
- 物理机可以从键盘、鼠标、磁盘噪声获取熵，VM 不行

**实现**:
```rust
// src/devices/src/virtio/rng/device.rs
pub struct Rng {
    rng: ThreadRng,  // 使用 host 的随机数源
}

// 处理 Guest 请求
fn process_request(&self) -> Vec<u8> {
    // 从 host 熵池生成随机数
    let mut random_bytes = vec![0u8; request_len];
    self.rng.fill_bytes(&mut random_bytes);
    random_bytes
}
```

#### Serial (串口)

**作用**: 模拟传统 UART 串口，用于早期控制台输出

**为什么需要？**:
- 内核早期调试（earlycon）
- 兼容传统软件
- 简单的调试接口

**LoongArch 实现**:
```rust
// src/devices/src/legacy/loongarch64/serial.rs
pub struct Serial {
    interrupt_enable: u8,
    interrupt_identification: u8,
    interrupt_evt: EventFd,
    line_control: u8,
    line_status: u8,
    in_buffer: VecDeque<u8>,   // 输入缓冲区
    out: Option<Box<dyn io::Write + Send>>,  // 输出（可重定向到文件）
    intc: Option<IrqChip>,     // 中断控制器
    irq_line: Option<u32>,     // 中断线
}
```

**寄存器模拟**:
- `DATA` (0x0): 数据寄存器
- `IER` (0x1): 中断使能寄存器
- `IIR` (0x2): 中断识别寄存器
- `LCR` (0x3): 线路控制寄存器
- `LSR` (0x5): 线路状态寄存器

---

### FDT (Flattened Device Tree) 详解

#### 什么是 FDT？

**FDT** = **Flattened Device Tree**（扁平设备树）

**作用**: 向内核描述硬件配置的数据结构

**为什么需要？**:
- x86 使用 ACPI，但 LoongArch/ARM 使用 FDT
- 内核启动时需要知道：有多少 CPU、内存布局、设备地址等
- FDT 是二进制格式，在启动时传递给内核

#### FDT 在内存中的位置

```
Guest 物理内存布局

0x4000_0000 (DRAM_MEM_START)
├─────────────────────┐
│                     │
│    Kernel Image     │  内核镜像
│                     │
├─────────────────────┤
│                     │
│       RAM           │  主内存
│                     │
├─────────────────────┤ ← ram_last_addr
│   initrd (如果有)   │
├─────────────────────┤
│   cmdline (16KB)    │  0xbffe8000
├─────────────────────┤
│ EFI System Table    │  0xbffec000
├─────────────────────┤
│   FDT (64KB)        │  0xbfff0000 ← fdt_addr
└─────────────────────┘
```

#### FDT 数据结构

```
FDT 二进制格式
┌─────────────────────────────────────┐
│ FDT Header (32 字节)                 │
│  - magic: 0xd00dfeed                │
│  - totalsize: FDT 总大小             │
│  - off_dt_struct: 结构体偏移         │
│  - off_dt_strings: 字符串表偏移      │
│  - off_mem_rsvmap: 保留内存映射偏移  │
├─────────────────────────────────────┤
│ Memory Reserve Map                  │
│  - 保留内存区域列表                  │
├─────────────────────────────────────┤
│ Structure Block                     │
│  - 节点树（嵌套结构）                │
│    ├── / (root)                    │
│    │   ├── cpus                    │
│    │   │   ├── cpu@0               │
│    │   │   └── cpu@1               │
│    │   ├── memory                  │
│    │   ├── chosen                  │
│    │   ├── interrupt-controller    │
│    │   └── virtio_mmio@XXXXXXXX    │
├─────────────────────────────────────┤
│ Strings Block                       │
│  - 节点名和属性名字符串              │
│  - "compatible", "reg", "interrupts"│
└─────────────────────────────────────┘
```

#### FDT 节点详解

```dts
/ {
    #address-cells = <2>;
    #size-cells = <2>;
    compatible = "libkrun,virt";

    // CPU 节点
    cpus {
        #address-cells = <1>;
        #size-cells = <0>;

        cpu@0 {
            device-type = "cpu";
            compatible = "loongson,la664";
            reg = <0>;  // CPU ID
        };
    };

    // 内存节点
    memory@40000000 {
        device-type = "memory";
        reg = <0x0 0x40000000  0x0 0x80000000>;
        //     地址高 32 位  地址低 32 位  大小高 32 位  大小低 32 位
        //     = 0x40000000 (1GB 起始)
        //     = 0x80000000 (2GB 大小)
    };

    // 启动参数节点
    chosen {
        bootargs = "console=hvc0 root=/dev/vda1 rw";
        stdout-path = "/virtio_mmio@4000000";
    };

    // 中断控制器节点
    interrupt-controller {
        compatible = "loongson,cpu-intc";
        interrupt-controller;
        #interrupt-cells = <1>;
    };

    // Virtio 设备节点
    virtio_mmio@4000000 {
        compatible = "virtio,mmio";
        reg = <0 0x4000000  0 0x200>;
        //  地址高  地址低    大小高  大小低
        interrupts = <5>;  // 中断号
        interrupt-parent = <&intc>;
    };
};
```

#### EFI System Table 详解

**什么是 EFI System Table？**:
- EFI (Extensible Firmware Interface) 系统表
- 提供系统信息和启动服务
- LoongArch 内核使用 EFI handoff 方式获取 FDT

**EFI System Table 布局**:
```
地址            内容
─────────────────────────────────────────
0xbffec000      EFI System Table Header
                - signature: "IBI SYS T"
                - revision: 2.10
                - nr_tables: 1
                - tables: 指向 Config Table
0xbffec100      EFI Config Table
                - guid: DEVICE_TREE_GUID
                - table: FDT 地址 (0xbfff0000)
0xbffec200      Vendor String "libkrun"
```

**内核如何使用**:
```c
// Linux 内核 arch/loongarch/kernel/efi.c
void __init efi_init(void)
{
    efi_system_table_t *systab;
    
    // 从 a2 寄存器获取 EFI System Table 地址
    systab = (efi_system_table_t *)boot_params->efi_system_table;
    
    // 查找 FDT 配置表
    for (i = 0; i < systab->nr_tables; i++) {
        if (guid_equal(&systab->tables[i].guid, &DEVICE_TREE_GUID)) {
            fdt_addr = systab->tables[i].table;
            //       ↑ 这就是 FDT 地址
            break;
        }
    }
    
    // 解析 FDT
    early_init_dt_scan(fdt_addr);
}
```

---

### VirtioFS 共享内存详解

#### 什么是 VirtioFS？

**VirtioFS** = **Virtio File System**

**作用**: 将 Host 文件系统目录共享给 Guest

**与传统方案对比**:
| 方案 | 性能 | 安全性 | 功能 |
|------|------|--------|------|
| 9p | 较慢 | 中等 | 完整文件系统语义 |
| VirtioFS | 快 | 高 | 完整文件系统语义 |
| virtio-blk | 快 | 高 | 块设备（文件镜像） |

#### VirtioFS 架构

```
┌─────────────────────────────────────────────────────────────────┐
│  Host (libkrun)                                                  │
│                                                                 │
│  ┌───────────────────────────────────────────────────────────┐ │
│  │  VirtioFS 设备 (src/devices/src/virtio/fs/)                │ │
│  │                                                           │ │
│  │  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐       │ │
│  │  │ FUSE Server │  │ Virtio Worker│  │ Passthrough │       │ │
│  │  │             │  │              │  │ (访问 Host FS)│      │ │
│  │  └─────────────┘  └─────────────┘  └─────────────┘       │ │
│  └───────────────────────────────────────────────────────────┘ │
│                                                                 │
│  共享目录：/home/yzw/rootfs_debian_unstable                     │
└─────────────────────────────────────────────────────────────────┘
                              ↕ Virtio Queue ↕
┌─────────────────────────────────────────────────────────────────┐
│  Guest (VM)                                                      │
│                                                                 │
│  ┌───────────────────────────────────────────────────────────┐ │
│  │  virtio-fs.ko (内核驱动)                                   │ │
│  │                                                           │ │
│  │  /dev/root (挂载为根文件系统)                              │ │
│  │      ↓                                                    │ │
│  │  virtiofs:/ (VirtioFS 挂载点)                              │ │
│  └───────────────────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────────────────┘
```

#### 共享内存 vs 文件系统共享

**重要澄清**: VirtioFS 共享的**不是**虚拟机的 RAM，而是**文件系统目录**

```
❌ 错误理解:
   VirtioFS 共享内存 = Guest 可以使用的 RAM
   
✅ 正确理解:
   VirtioFS 共享目录 = Host 的某个目录在 Guest 中可见
                      Guest 可以像访问本地文件一样访问这些文件
```

**示例**:
```bash
# Host 启动命令
./smoke_kernel vmlinuz.efi ./rootfs_debian_unstable
#                                ↑ 这个目录通过 VirtioFS 共享

# Guest 内部看到的
/ (根文件系统)
├── bin/        ← 实际在 Host 的 ./rootfs_debian_unstable/bin/
├── etc/        ← 实际在 Host 的 ./rootfs_debian_unstable/etc/
├── home/       ← 实际在 Host 的 ./rootfs_debian_unstable/home/
└── ...
```

#### VirtioFS 工作原理

**FUSE 协议**:
```
Guest 应用: open("/etc/passwd")
    ↓
Guest VFS: 路由到 virtio-fs 文件系统
    ↓
VirtioFS 驱动：构建 FUSE 请求
    ↓
Virtio Queue: 将请求放入共享队列
    ↓
中断通知：触发中断通知 Host
    ↓
Host VirtioFS: 从队列读取请求
    ↓
Passthrough: 访问 Host 文件系统 (open("./rootfs/etc/passwd"))
    ↓
Host VirtioFS: 将结果放入响应队列
    ↓
中断通知：触发中断通知 Guest
    ↓
Guest VFS: 读取响应，返回给应用
```

#### SHM (Shared Memory) 区域

**SHM 区域**是另一块内存，用于 VirtioFS 的高效数据传输：

```rust
// src/arch/src/loongarch64/mod.rs
let shm_start_addr = ((ram_last_addr / 0x4000_0000) + 1) * 0x4000_0000;
// 按 1GB 对齐，位于 RAM 之后
```

**SHM 用途**:
- DAX (Direct Access): Guest 直接访问共享内存，无需复制
- 零拷贝 I/O: 大文件传输时避免内存复制
- 页缓存共享: Host 和 Guest 共享页缓存

**SHM 不是 Guest RAM**:
- Guest 不能像普通 RAM 那样随意使用 SHM
- SHM 专门用于 VirtioFS 等设备的优化传输
- 由 VMM 管理和映射

## 阶段 12：IOCSR 模拟（LoongArch 特有）

### #19  handle_iocsr_read / process_iocsr_read
```
文件：src/vmm/src/linux/vstate.rs
行号：~1563
```

**作用**: 处理 Guest 对 IOCSR 寄存器的读请求（模拟硬件行为）

**调用栈**:
```
#19 Vcpu::handle_iocsr_read / process_iocsr_read (
        &mut self,
        addr: u64,                     # 输入：IOCSR 寄存器地址
        data: &mut [u8],               # 输出：读取的数据缓冲区
    ) -> Result<()>                    # 输出：成功/失败
    at src/vmm/src/linux/vstate.rs:1563
```

**为什么需要模拟 IOCSR？**:
- KVM 不会自动处理 IOCSR 访问
- Guest 内核会读取 IOCSR 获取 CPU 信息
- Userspace 必须模拟这些寄存器返回合理的值

**关键代码**:
```rust
fn handle_iocsr_read(&mut self, addr: u64, data: &mut [u8]) -> Result<()> {
    match addr {
        // === LOONGARCH_IOCSR_CPUID (0x8) ===
        0x8 => {
            // 返回 CPU 中断特性
            // 0x818 = 支持定时器 (0x8) + IPI (0x10) + 性能计数器 (0x800)
            let value = 0x818u64;
            data.copy_from_slice(&value.to_le_bytes()[..data.len()]);
            debug!("LoongArch IOCSR read: addr=0x{:x}, len={}, value=0x{:x}",
                   addr, data.len(), value);
        }
        
        // === LOONGARCH_IOCSR_VENDOR (0x10) ===
        0x10 => {
            // 返回厂商标识
            // KVM Guest 返回 0，物理 CPU 返回 "Loongson"
            let value = 0u64;
            data.copy_from_slice(&value.to_le_bytes()[..data.len()]);
            debug!("LoongArch IOCSR read: addr=0x{:x}, len={}, value=0x{:x}",
                   addr, data.len(), value);
        }
        
        // === LOONGARCH_IOCSR_FEATURES (0x20) ===
        0x20 => {
            // 返回特性标识
            // KVM Guest 返回 0 或 "KVMGuest"
            let value = 0u64;
            data.copy_from_slice(&value.to_le_bytes()[..data.len()]);
            debug!("LoongArch IOCSR read: addr=0x{:x}, len={}, value=0x{:x}",
                   addr, data.len(), value);
        }
        
        // === 其他地址 ===
        _ => {
            // 未模拟的寄存器返回 0
        }
    }
    Ok(())
}
```

**为什么 match 的是 0x8、0x10、0x20？**:
这些是 LoongArch 架构定义的 IOCSR 寄存器**偏移地址**：
- `0x8`: CPUID 寄存器偏移
- `0x10`: VENDOR 寄存器偏移  
- `0x20`: FEATURES 寄存器偏移

完整 IOCSR 地址空间是连续的，CPU 通过 `iocsrc rd, offset` 指令访问。

**日志输出**:
```
[2026-03-24T03:12:56.096035Z DEBUG vmm::linux::vstate]
    LoongArch IOCSR read: addr=0x8, len=4, value=0x818
    # Guest 读取 CPUID，返回 0x818（中断特性）
    
[2026-03-24T03:12:56.096098Z DEBUG vmm::linux::vstate]
    LoongArch IOCSR read: addr=0x10, len=8, value=0x0
    # Guest 读取厂商标识，返回 0（KVM Guest）
    
[2026-03-24T03:12:56.096106Z DEBUG vmm::linux::vstate]
    LoongArch IOCSR read: addr=0x20, len=8, value=0x0
    # Guest 读取特性标识，返回 0（KVM Guest）
```

**Guest 内核如何使用这些值**:
```c
// Linux 内核 arch/loongarch/kernel/cpu-probe.c
void cpu_probe(void) {
    // 读取 CPUID
    cpucfg0 = iocsrrd_d(CSR_CPUID);
    
    // 根据 CPUID 识别 CPU 型号
    if ((cpucfg0 & 0xffff) == 0xd010) {
        // Loongson-64bit
        cpu_name = "Loongson-64bit";
    }
    
    // 读取厂商标识
    vendor = iocsrrd_d(CSR_VENDOR);
    if (vendor == 0) {
        // KVM 虚拟化环境
        pr_info("Running in KVM guest\n");
    }
}
```

---

## 阶段 13：Guest 内核启动

### Guest 内核启动流程

```
┌─────────────────────────────────────────────────────────────────────────────┐
│  Guest 内核启动 (在 KVM 中执行)                                               │
├─────────────────────────────────────────────────────────────────────────────┤
│  #0  _start (arch/loongarch/kernel/head.S)                                  │
│        │ PC = 0x41660000 (内核入口)                                          │
│        │ a0 = 1 (EFI boot)                                                   │
│        │ a1 = 0xbffe8000 (命令行地址)                                        │
│        │ a2 = 0xbffec000 (EFI System Table 地址)                              │
│        │                                                                     │
│        #1  efi_init()                                                        │
│              │ 从 a2 获取 EFI System Table                                    │
│              │ 解析 EFI Config Table                                          │
│              │ 通过 DEVICE_TREE_GUID 获取 FDT 地址 0xbfff0000                  │
│              │                                                               │
│              #2  early_init_dt_scan(fdt_addr)                                │
│                    │ 解析 FDT                                                 │
│                    ├── 解析 CPU 节点 → 获取 CPU 数量                            │
│                    ├── 解析内存节点 → 获取内存布局                            │
│                    ├── 解析 chosen 节点 → 获取启动参数                        │
│                    ├── 解析中断控制器节点 → 建立中断层级                      │
│                    └── 解析设备节点 → 注册设备驱动                            │
│                    │                                                          │
│                    #3  setup_arch()                                          │
│                          │ 架构特定初始化                                     │
│                          ├── setup_memory()                                   │
│                          ├── setup_cmdline()                                  │
│                          └── early_ioremap_init()                             │
│                          │                                                    │
│                          #4  start_kernel()                                  │
│                                │ 内核主初始化                                 │
│                                ├── setup_log_buf()                            │
│                                ├── vfs_caches_init()                          │
│                                ├── page_cache_init()                          │
│                                ├── init_IRQ()                                 │
│                                ├── sched_init()                               │
│                                ├── time_init()                                │
│                                ├── console_init()                             │
│                                │      │                                       │
│                                │      #5  uart8250_early_console_write()     │
│                                │            │  earlycon 输出                  │
│                                │            │                                 │
│                                │            [    0.000000] Linux version... │
│                                │                                              │
│                                ├── rest_init()                                │
│                                │      │                                       │
│                                │      #6  kernel_thread(init, ...)           │
│                                │            │ 启动 init 进程                   │
│                                │            │                                 │
│                                │            #7  init_main()                  │
│                                │                  │                           │
│                                │                  #8  mount_root()           │
│                                │                        │ VirtioFS 挂载       │
│                                │                        │                     │
│                                │                        [    1.249561]      │
│                                │                        VFS: Mounted root  │
│                                │                                              │
│                                └── cpu_startup_entry()                        │
│                                      │ 进入空闲循环                           │
└─────────────────────────────────────────────────────────────────────────────┘
```

---

## 阶段 14：中断注入流程

### 设备中断注入（以 virtio-fs 为例）

```
┌─────────────────────────────────────────────────────────────────────────────┐
│  中断注入流程 (virtio-fs)                                                    │
├─────────────────────────────────────────────────────────────────────────────┤
│  #0  devices::virtio::fs::worker                                            │
│        │ 处理 FUSE 请求                                                       │
│        │                                                                     │
│        #1  mmio[fs].interrupt: signal_used_queue                            │
│              at src/devices/src/virtio/mmio.rs:141                          │
│              │ 设置 interrupt status                                         │
│              │                                                               │
│              #2  irqchip.set_irq_state(irq=6, active=true)                  │
│                    at src/devices/src/legacy/kvmloongarchirqchip.rs:116     │
│                    │                                                         │
│                    #3  KVM_INTERRUPT ioctl (signed_irq=+6)                  │
│                          at src/devices/src/legacy/kvmloongarchirqchip.rs:145│
│                          │                                                   │
│                          [DEBUG] KVM_INTERRUPT ok: irq=6, signed_irq=6,     │
│                                  active=true                                │
├─────────────────────────────────────────────────────────────────────────────┤
│  KVM (kernel)                                                                │
│        #4  kvm_vcpu_ioctl(KVM_INTERRUPT)                                    │
│              at arch/loongarch/kvm/vcpu.c:847                               │
│              │                                                               │
│              #5  kvm_queue_irq(vcpu, intr=6)                                │
│                    │ 设置 CPU pending HWI6                                   │
├─────────────────────────────────────────────────────────────────────────────┤
│  Guest Linux                                                                 │
│        #6  CPU 检测到 HWI6 pending                                           │
│              │                                                               │
│              #7  do_IRQ(irq=19)  // FDT 映射后                              │
│                    at drivers/irqchip/irq-loongarch-cpu.c                   │
│                    │                                                         │
│                    #8  generic_handle_domain_irq()                          │
│                          │                                                   │
│                          #9  virtio_mmio_irqhandler()                       │
│                                at drivers/virtio/virtio_mmio.c              │
│                                │                                             │
│                                #10 vm_interrupt(irq=19)                     │
│                                      │                                       │
│                                      [    1.249561]                          │
│                                      virtio-mmio: vm_interrupt irq=19       │
│                                                                              │
│                                      #11 read interrupt status: 0x1         │
│                                      #12 write interrupt ack: 0x1           │
│                                                                              │
│                                      #13 irqchip.set_irq_state(active=false)│
│                                            │                                 │
│                                            [DEBUG] KVM_INTERRUPT ok:        │
│                                                    irq=6, signed_irq=-6,    │
│                                                    active=false             │
└─────────────────────────────────────────────────────────────────────────────┘
```

### 关键函数详解

#### signal_used_queue 完整实现

```rust
// src/devices/src/virtio/mmio.rs
pub fn signal_used_queue(&self) {
    // 尝试信号中断，如果失败则记录警告
    if let Err(e) = self.try_signal_used_queue() {
        warn!(target: &self.0.log_target, "Failed to signal used queue: {e:?}");
    }
}

pub fn try_signal_used_queue(&self) -> Result<(), crate::Error> {
    debug!(target: &self.0.log_target, "interrupt: signal_used_queue");
    // 调用 try_signal 并传入 VRING 中断位
    self.try_signal(VIRTIO_MMIO_INT_VRING)
}

fn try_signal(&self, status: u32) -> Result<(), crate::Error> {
    #[cfg(target_arch = "loongarch64")]
    {
        // 获取中断同步锁（防止并发中断）
        let _irq_sync = self.0.irq_sync.lock().unwrap();
        
        // 原子操作：设置中断状态位，返回旧值
        let old = self.status().fetch_or(status as usize, Ordering::SeqCst);
        
        // 关键逻辑：只在状态从 0 变非 0 时触发中断
        if old == 0 {
            // 调用中断控制器的 set_irq_state
            self.intc()
                .lock()
                .unwrap()
                .set_irq_state(
                    self.0.irq_line,      // 中断线号（如 irq=6）
                    Some(&self.0.event),  // 事件文件描述符
                    true                  // active=true（断言中断）
                )?;
        }
        return Ok(());
    }

    // 其他架构使用不同的中断路径
    #[cfg(not(target_arch = "loongarch64"))]
    {
        self.status().fetch_or(status as usize, Ordering::SeqCst);
        self.intc()
            .lock()
            .unwrap()
            .set_irq(self.0.irq_line, Some(&self.0.event))?;
        Ok(())
    }
}
```

**为什么 LoongArch 需要特殊处理？**:
1. **电平触发语义**: 使用 `set_irq_state` 而不是 `set_irq`
2. **避免重复中断**: 检查 `old == 0` 才触发
3. **同步锁**: `_irq_sync` 防止并发中断注入

#### set_irq_state 完整实现

```rust
// src/devices/src/legacy/kvmloongarchirqchip.rs
impl IrqChipT for KvmLoongArchIrqChip {
    fn set_irq_state(
        &self,
        irq_line: Option<u32>,
        _interrupt_evt: Option<&EventFd>,
        active: bool,
    ) -> Result<(), DeviceError> {
        // 1. 获取中断线号
        let irq = match irq_line {
            Some(irq) => irq,
            None => {
                return Err(DeviceError::FailedSignalingUsedQueue(
                    io::Error::new(io::ErrorKind::InvalidInput, "irq_line not set")
                ));
            }
        };

        // 2. 计算有符号中断号（关键！）
        let signed_irq = if active {
            irq as i32       // 正数 = 断言中断
        } else {
            -(irq as i32)    // 负数 = 撤销中断
        };

        // 3. 构建 KVM 中断结构
        let interrupt = kvm_interrupt {
            // KVM uAPI 暴露 irq 为 u32，但 LoongArch KVM 会转回 int
            // 符号位用于区分 assert/deassert
            irq: signed_irq as u32,
        };

        // 4. 调用 KVM_INTERRUPT ioctl
        let ret = unsafe {
            ioctl_with_ref(&self.irq_vcpu_fd, KVM_INTERRUPT_LOONGARCH(), &interrupt)
        };
        
        if ret != 0 {
            let e = io::Error::last_os_error();
            error!(
                "KVM_INTERRUPT failed: irq={}, signed_irq={}, active={}, err={e:?}",
                irq, signed_irq, active
            );
            return Err(DeviceError::FailedSignalingUsedQueue(e));
        }

        // 5. 成功日志
        debug!(
            "KVM_INTERRUPT ok: irq={}, signed_irq={}, active={}",
            irq, signed_irq, active
        );
        Ok(())
    }
}
```

**signed_irq 转换逻辑**:
```rust
let signed_irq = if active {
    irq as i32       // 例如：irq=6 → signed_irq=+6
} else {
    -(irq as i32)    // 例如：irq=6 → signed_irq=-6
};
```

**为什么这样转换？**:
- KVM LoongArch 的 `kvm_queue_irq()` 会检查 `intr` 的符号
- 正数：调用 `kvm_set_irq(vcpu, intr)` → 设置 pending
- 负数：调用 `kvm_dequeue_irq(vcpu, -intr)` → 清除 pending

#### Guest 中断处理流程

```c
// Guest 内核：drivers/virtio/virtio_mmio.c
static irqreturn_t virtio_mmio_irqhandler(int irq, void *priv)
{
    struct virtio_mmio_device *vm_dev = priv;
    u32 status;
    
    // 步骤 1: 读取中断状态寄存器 (0x010)
    status = readl(vm_dev->base + VIRTIO_MMIO_INTERRUPT_STATUS);
    
    // 步骤 2: 检查是否有有效中断
    if (!status)
        return IRQ_NONE;
    
    // 步骤 3: 调用 virtio 核心处理
    vring_interrupt(irq, vm_dev->vdev);
    //         ↑ 这个函数会处理 used ring 中的完成队列
    
    // 步骤 4: 返回中断处理结果
    return IRQ_HANDLED;
}

// virtio 核心：drivers/virtio/virtio_ring.c
irqreturn_t vring_interrupt(int irq, void *private)
{
    struct virtqueue *vq = private;
    
    // 回调设备驱动的回调函数
    if (vq->callback)
        vq->callback(vq);
    //       ↑ 例如：virtio_fs_done() 通知 FUSE 层 I/O 完成
    
    return IRQ_HANDLED;
}
```

#### ACK 操作详解

```c
// Guest 内核：drivers/virtio/virtio_mmio.c
static void virtio_mmio_ack_interrupt(struct virtio_mmio_device *vm_dev, u32 status)
{
    // 写入中断 ACK 寄存器 (0x014)
    // 这个写操作会清除中断挂起状态
    writel(status, vm_dev->base + VIRTIO_MMIO_INTERRUPT_ACK);
    //      ↑             ↑
    //    写入值        ACK 寄存器偏移
}

// 在 vring_interrupt() 中调用
static irqreturn_t virtio_mmio_irqhandler(int irq, void *priv)
{
    struct virtio_mmio_device *vm_dev = priv;
    u32 status;
    
    status = readl(vm_dev->base + VIRTIO_MMIO_INTERRUPT_STATUS);
    
    if (!status)
        return IRQ_NONE;
    
    vring_interrupt(irq, vm_dev->vdev);
    
    // 处理完成后 ACK
    virtio_mmio_ack_interrupt(vm_dev, status);
    //                        ↑ 这就是 "write interrupt ack: 0x1"
    
    return IRQ_HANDLED;
}
```

**为什么需要 ACK？**:
```
时间线：
T0: Host 完成 I/O → signal_used_queue() → 触发中断
T1: Guest 收到中断 → vm_interrupt() → 处理 used ring
T2: Guest 写入 ACK → 清除中断状态
T3: Host 检测到 ACK → deassert 中断线 → 准备下一次中断

如果没有 ACK：
- 中断状态寄存器保持为 1
- 中断线持续为高电平
- Guest 会不断收到中断（中断风暴）
```

#### Deassert 流程

```rust
// Guest 写入 ACK 后，Host 侧的响应（简化版）
fn write_interrupt_ack(&self, value: u32) {
    // 清除中断状态
    self.status.fetch_and(!value as usize, Ordering::SeqCst);
    
    // 如果状态变为 0，撤销中断
    if self.status.load(Ordering::SeqCst) == 0 {
        self.intc()
            .lock()
            .unwrap()
            .set_irq_state(self.irq_line, None, false)?;
            //                                   ↑ active=false → signed_irq=-6
    }
}
```

**日志输出**:
```
[2026-03-24T03:12:57.589054Z DEBUG devices::virtio::mmio[fs]]
    write interrupt ack: 0x1
    # Guest 写入 ACK，清除 VRING 中断位
    
[2026-03-24T03:12:57.589059Z DEBUG devices::legacy::kvmloongarchirqchip]
    KVM_INTERRUPT ok: irq=6, signed_irq=-6, active=false
    # Host 检测到状态为 0，撤销中断（signed_irq=-6）
```

---

## 阶段 15：VirtioFS 挂载成功

### FUSE 初始化与根文件系统挂载

**日志输出**:
```
[    0.620841][    T1] fuse: init (API version 7.40)
[    0.631245][    T1] virtio-mmio: virtio-mmio: find_vqs irq=19 nvqs=2 dev=26
[    0.632776][    T1] Serial: 8250/16550 driver, 16 ports, IRQ sharing enabled
[    0.634105][    T1] virtio-mmio: virtio-mmio: find_vqs irq=20 nvqs=8 dev=3
[    1.249561][    C0] virtio-fs: vq_done name=requests.0
[    1.249807][   T58] virtio-fs: get_buf req=0000000000000000 len=0
[    1.250096][   T58] virtio-fs done: req=0000000000000000 len=0
[    1.250410][   T58] VFS: Mounted root (virtiofs filesystem)
```

---

## 关键时间节点

| 时间戳 | 阶段 | 函数 | 说明 |
|--------|------|------|------|
| T0 03:12:55.218 | 载荷选择 | `choose_payload` | 识别 PE/GZ 格式 |
| T1 03:12:55.453 | 内核加载 | `load_external_kernel` | 解压并加载内核 |
| T2 03:12:55.467 | CPUCFG 初始化 | `setup_cpucfg` | 设置 CPU 特性寄存器 |
| T3 03:12:55.467 | 寄存器配置 | `setup_regs` | 设置 PC/a0/a1/a2 |
| T4 03:12:56.094 | FDT 创建 | `create_fdt` | 创建设备树 |
| T5 03:12:56.094 | EFI 设置 | `setup_fdt_system_table` | 创建 EFI System Table |
| T6 03:12:56.096 | vCPU 启动 | `Vcpu::run` | 开始执行 KVM_RUN |
| T7 03:12:56.096 | IOCSR 读取 | `handle_iocsr_read` | Guest 读取 CPU 信息 |
| T8 03:12:57.006 | 控制台初始化 | `virtio_console` | Virtio 控制台就绪 |
| T9 03:12:57.588 | VirtioFS 中断 | `KVM_INTERRUPT` | 第一次 FS 中断注入 |
| T10 03:12:57.620 | 根文件系统挂载 | `VFS: Mounted root` | VirtioFS 挂载成功 |

---

## 内存布局详解

### Guest 物理内存布局

```
物理地址              大小         用途
─────────────────────────────────────────────────────────
0x0000_0000          160MB       保留区（MMIO 设备）
0x0A00_0000          16KB        串口 0 (0xa001000)
0x0A00_1000          4KB         PCHPIC (0x10000000)
0x0A00_2000          5*4KB       Virtio MMIO 设备
  ├─ 0xa002000       balloon     (irq=3)
  ├─ 0xa003000       rng         (irq=4)
  ├─ 0xa004000       console     (irq=5)
  ├─ 0xa005000       fs          (irq=6)
  └─ 0xa006000       vsock       (irq=7)
0x4000_0000          ~2GB        主内存 (DRAM)
  ├─ 0x40200000      ~22MB       内核镜像
  └─ 0x41660000      -           内核入口点
0xbffe8000           16KB        内核命令行
0xbffec000           4KB         EFI System Table
0xbfff0000           64KB        FDT 设备树
0xc0000000           -           RAM 末尾
```

---

## 中断层级结构

```
CPU INTC (phandle=1, onecell)
├── loongson,cpu-interrupt-controller
├── #interrupt-cells = <1>
└── 直接连接设备中断
    ├── Serial (irq=2)  → HWI2
    ├── Virtio Balloon (irq=3) → HWI3
    ├── Virtio Rng (irq=4) → HWI4
    ├── Virtio Console (irq=5) → HWI5
    ├── Virtio FS (irq=6) → HWI6
    └── Virtio Vsock (irq=7) → HWI7
```

**注**: 当前实现使用 `cpuintc` 直连模型，绕过了传统的 `PCH-PIC/EIOINTC` 层级。

---

## 参考资料

### 源码文件

| 文件路径 | 说明 |
|----------|------|
| `src/libkrun/src/lib.rs` | libkrun C API 入口 |
| `src/vmm/src/builder.rs` | VM 构建主流程 |
| `src/vmm/src/vmm.rs` | VMM 核心逻辑 |
| `src/vmm/src/linux/vstate.rs` | vCPU 状态管理 |
| `src/arch/src/loongarch64/mod.rs` | LoongArch 架构抽象 |
| `src/arch/src/loongarch64/layout.rs` | 内存布局定义 |
| `src/arch/src/loongarch64/linux/regs.rs` | 寄存器配置 |
| `src/arch/src/loongarch64/linux/efi.rs` | EFI System Table |
| `src/devices/src/fdt/loongarch64.rs` | FDT 创建 |
| `src/devices/src/legacy/kvmloongarchirqchip.rs` | 中断控制器 |
| `src/devices/src/virtio/mmio.rs` | Virtio MMIO 传输 |

### 内核文档

- [LoongArch KVM 内核文档](https://www.kernel.org/doc/html/latest/virt/kvm/loongarch.html)
- [LoongArch 架构手册](https://loongson.github.io/LoongArch-Documentation/)
- [Device Tree 规范](https://devicetree-specification.readthedocs.io/)

---

## 附录 A：关键数据结构与常量

### Virtio MMIO 寄存器布局

```
偏移      名称                        宽度    说明
───────────────────────────────────────────────────────────────────
0x000     VIRTIO_MMIO_MAGIC           32 位   魔数 (0x74726976)
0x004     VIRTIO_MMIO_VERSION         32 位   版本号
0x008     VIRTIO_MMIO_DEVICE_ID       32 位   设备 ID
0x010     VIRTIO_MMIO_INTERRUPT_STATUS 32 位  中断状态寄存器 (只读)
0x014     VIRTIO_MMIO_INTERRUPT_ACK   32 位   中断 ACK 寄存器 (只写)
0x020     VIRTIO_MMIO_QUEUE_SEL       32 位   队列选择寄存器
0x024     VIRTIO_MMIO_QUEUE_NUM_MAX   32 位   队列最大大小
0x028     VIRTIO_MMIO_QUEUE_NUM       32 位   队列当前大小
0x030     VIRTIO_MMIO_QUEUE_READY     32 位   队列就绪标志
0x034     VIRTIO_MMIO_QUEUE_NOTIFY    32 位   队列通知寄存器
0x044     VIRTIO_MMIO_INTERRUPT_STATUS_EX 32 位 扩展中断状态
0x048     VIRTIO_MMIO_INTERRUPT_ACK_EX 32 位  扩展中断 ACK
0x100     VIRTIO_MMIO_QUEUE_DESC_LOW  32 位   描述表地址 (低 32 位)
0x104     VIRTIO_MMIO_QUEUE_DESC_HIGH 32 位   描述表地址 (高 32 位)
0x108     VIRTIO_MMIO_QUEUE_DRIVER_LOW 32 位  驱动环地址 (低 32 位)
0x10C     VIRTIO_MMIO_QUEUE_DRIVER_HIGH 32 位 驱动环地址 (高 32 位)
0x110     VIRTIO_MMIO_QUEUE_DEVICE_LOW 32 位  设备环地址 (低 32 位)
0x114     VIRTIO_MMIO_QUEUE_DEVICE_HIGH 32 位 设备环地址 (高 32 位)
```

### 中断状态位定义

```rust
// Virtio MMIO 中断状态位
const VIRTIO_MMIO_INT_VRING: u32 = 0x1;   // 队列中断 (used ring 更新)
const VIRTIO_MMIO_INT_CONFIG: u32 = 0x2;  // 配置变更中断

// 日志中的值含义
0x1  → VIRTIO_MMIO_INT_VRING  → "write interrupt ack: 0x1"
0x2  → VIRTIO_MMIO_INT_CONFIG → "write interrupt ack: 0x2"
0x3  → 两者都有              → "write interrupt ack: 0x3"
```

### KVM 设备类型常量

```rust
// LoongArch KVM 设备类型 (kvm_bindings::kvm_device_type)
const KVM_DEV_TYPE_LOONGARCH_IPI: u32 = 5;     // IPI 设备
const KVM_DEV_TYPE_LOONGARCH_EIOINTC: u32 = 6; // EIOINTC 设备
const KVM_DEV_TYPE_LOONGARCH_PCHPIC: u32 = 7;  // PCHPIC 设备

// 设备属性组
const KVM_DEV_LOONGARCH_EXTIOI_GRP_CTRL: u64 = 1;  // EIOINTC 控制组
const KVM_DEV_LOONGARCH_PCH_PIC_GRP_CTRL: u64 = 1; // PCHPIC 控制组

// 设备属性
const KVM_DEV_LOONGARCH_EXTIOI_CTRL_INIT_NUM_CPU: u64 = 1;  // 初始化 CPU 数量
const KVM_DEV_LOONGARCH_EXTIOI_CTRL_INIT_FEATURE: u64 = 2;  // 初始化特性
const KVM_DEV_LOONGARCH_PCH_PIC_CTRL_INIT: u64 = 1;         // PCHPIC 初始化
```

### ioctl 常量

```rust
// KVM ioctl 命令
const KVM_CREATE_VM: u32 = 0xAE01;      // 创建 VM
const KVM_CREATE_VCPU: u32 = 0xAE41;    // 创建 vCPU
const KVM_SET_USER_MEMORY_REGION: u32 = 0x4020AE46;  // 设置内存区域
const KVM_SET_DEVICE_ATTR: u32 = 0x4018AE4D;         // 设置设备属性
const KVM_INTERRUPT: u32 = 0x8004AE43;  // 中断注入 (LoongArch)

// LoongArch 特定的 ioctl
const KVM_GET_REGS: u32 = 0x8108AE48;   // 获取寄存器
const KVM_SET_REGS: u32 = 0x4108AE49;   // 设置寄存器
const KVM_GET_ONE_REG: u32 = 0x4010AE4B; // 获取单个寄存器
const KVM_SET_ONE_REG: u32 = 0x4010AE4C; // 设置单个寄存器
```

---

## 附录 B：常见问题解答 (FAQ)

### Q1: vCPU 线程和普通线程有什么区别？

**A**: vCPU 线程是执行 `KVM_RUN` ioctl 的特殊线程：

```rust
// 普通线程
fn normal_thread() {
    loop {
        // 执行普通 Rust 代码
    }
}

// vCPU 线程
fn vcpu_thread() {
    loop {
        // KVM_RUN 让 CPU 进入 non-root 模式执行 Guest 代码
        match kvm_vcpu.run() {  // ← 这里会切换到 Guest
            // VM Exit 后在这里处理
            VcpuExit::MmioRead => { /* 处理 MMIO */ }
            VcpuExit::IocsrRead => { /* 处理 IOCSR */ }
        }
    }
}
```

### Q2: MMIO 设备是在内核还是用户空间处理？

**A**: MMIO 设备模拟**完全在用户空间**：

```
Guest 访问 MMIO 地址
    ↓
VM Exit (硬件自动，进入 KVM 内核)
    ↓
KVM_RUN 返回到用户空间
    ↓
VMM 的 mmio_bus.read/write() 处理  ← 用户空间
    ↓
KVM_RUN 再次进入 Guest
```

**为什么这样设计？**:
- 灵活性：设备逻辑用 Rust 编写，易于维护
- 安全性：设备崩溃不影响内核稳定性
- 性能：大部分 MMIO 操作很快，user/kernel 切换开销可接受

### Q3: VirtioFS 共享的目录是 Guest 的内存吗？

**A**: **不是**。VirtioFS 共享的是**文件系统**，不是 RAM。

```
Host: /home/user/rootfs/  ← 这是一个目录
           ↓ VirtioFS 共享
Guest: / (根文件系统)      ← Guest 看到的是文件系统树

Guest 访问 /etc/passwd
    ↓
VirtioFS 驱动通过 virtio queue 发送请求
    ↓
Host VirtioFS 访问 /home/user/rootfs/etc/passwd
    ↓
返回文件内容给 Guest
```

**SHM 区域**是用于优化的共享内存，但 Guest 不能像普通 RAM 那样随意使用它。

### Q4: 为什么需要 FDT 和 EFI System Table 两个东西？

**A**: 它们有不同的用途：

| | FDT | EFI System Table |
|---|-----|------------------|
| **作用** | 描述硬件配置 | 提供启动服务和运行时服务 |
| **格式** | 扁平二进制树 | 标准 EFI 结构 |
| **LoongArch 用途** | 内核解析设备信息 | 获取 FDT 地址的入口点 |

**启动流程**:
```
CPU 启动
    ↓
a2 寄存器 → EFI System Table
    ↓
解析 EFI Config Table
    ↓
通过 DEVICE_TREE_GUID 找到 FDT 地址
    ↓
解析 FDT 获取硬件信息
```

### Q5: EventFd 和信号量有什么区别？

**A**: EventFd 是简化的计数信号量：

| | EventFd | 信号量 |
|---|---------|--------|
| **操作** | write(值) 加计数，read() 读取并清零 | P/V 操作 |
| **用途** | 事件通知 | 资源计数/互斥 |
| **性能** | 更轻量 | 相对较重 |

```rust
// EventFd 典型用法
let efd = EventFd::new(0)?;

// 线程 A：等待事件
efd.read()?;  // 阻塞直到有事件
println!("事件发生!");

// 线程 B：触发事件
efd.write(1)?;  // 计数 +1，唤醒等待者
```

---

## 参考资料

### 规范文档

- [LoongArch 架构手册](https://loongson.github.io/LoongArch-Documentation/)
- [LoongArch 虚拟化扩展](https://loongson.github.io/LoongArch-Documentation/)
- [KVM LoongArch 内核文档](https://www.kernel.org/doc/html/latest/virt/kvm/loongarch.html)
- [Virtio 规范](https://docs.oasis-open.org/virtio/virtio/v1.1/virtio-v1.1.html)
- [Device Tree 规范](https://devicetree-specification.readthedocs.io/)
- [EFI 规范](https://uefi.org/specifications)

### 内核源码

| 文件路径 | 说明 |
|----------|------|
| `arch/loongarch/kvm/vcpu.c` | KVM vCPU 中断处理 |
| `arch/loongarch/kvm/vm.c` | KVM VM ioctl 实现 |
| `arch/loongarch/include/asm/loongarch.h` | LoongArch 架构定义 |
| `arch/loongarch/include/asm/iocsr.h` | IOCSR 寄存器定义 |
| `drivers/irqchip/irq-loongarch-cpu.c` | CPU interrupt controller 驱动 |
| `drivers/virtio/virtio_mmio.c` | Virtio MMIO 驱动 |
| `drivers/virtio/virtio_ring.c` | Virtio 环处理 |
| `drivers/virtio/virtio_fs.c` | VirtioFS 驱动 |
| `arch/loongarch/kernel/efi.c` | EFI 初始化 |
| `arch/loongarch/kernel/cpu-probe.c` | CPU 探测和识别 |

### libkrun 源码

| 文件路径 | 说明 |
|----------|------|
| `src/libkrun/src/lib.rs` | libkrun C API 入口 |
| `src/vmm/src/builder.rs` | VM 构建流程 |
| `src/vmm/src/vmm.rs` | VMM 核心结构 |
| `src/vmm/src/linux/vstate.rs` | vCPU 状态管理和运行循环 |
| `src/arch/src/loongarch64/layout.rs` | IRQ 编号和内存布局定义 |
| `src/arch/src/loongarch64/linux/regs.rs` | 寄存器配置 (CPUCFG/PC/a0-a2) |
| `src/arch/src/loongarch64/linux/efi.rs` | EFI System Table 创建 |
| `src/arch/src/loongarch64/linux/iocsr.rs` | IOCSR 模拟 |
| `src/devices/src/fdt/loongarch64.rs` | FDT 设备树创建 |
| `src/devices/src/legacy/irqchip.rs` | irqchip 抽象接口 |
| `src/devices/src/legacy/kvmloongarchirqchip.rs` | KVM LoongArch 中断控制器 |
| `src/devices/src/legacy/loongarch64/serial.rs` | 串口设备模拟 |
| `src/devices/src/virtio/mmio.rs` | virtio-mmio 传输层和中断逻辑 |
| `src/devices/src/virtio/balloon/device.rs` | 内存气球设备 |
| `src/devices/src/virtio/rng/device.rs` | 随机数生成器 |
| `src/devices/src/virtio/fs/` | VirtioFS 设备（含 FUSE server）|
| `src/devices/src/virtio/console/` | Virtio 控制台设备 |

### 相关文档

- [LOONGARCH_PORTING.md](LOONGARCH_PORTING.md) - LoongArch64 架构适配文档
- [LOONGARCH_INTERRUPT_ANALYSIS.md](LOONGARCH_INTERRUPT_ANALYSIS.md) - LoongArch 中断问题分析

---

*最后更新：2026 年 3 月 24 日*

---

## 附录 C: KVM 虚拟化核心机制详解

本节深入讲解 KVM 虚拟化的三个核心机制：内存虚拟化、中断虚拟化、寄存器虚拟化，以及 Guest 内核驱动的运行机制。

### 1. 内存虚拟化 (Memory Virtualization) - 深入详解

#### 你的疑问解答

**问题 1**: 为什么要遍历注册？不是只注册一大块吗？

**回答**: 是的，通常只注册一个连续区域，但代码设计支持多个不连续区域。

```rust
// src/vmm/src/builder.rs
fn create_guest_memory(
    mem_size: usize,
    initrd_size: u64,
) -> Result<(GuestMemoryMmap, ArchMemoryInfo)> {
    // 1. 计算内存布局
    let (arch_memory_info, regions) = arch::loongarch64::arch_memory_regions(
        mem_size,
        initrd_size,
        None,
    );
    
    // regions = [(GuestAddress(0x40000000), 2GB)]
    // LoongArch 通常只有一个连续区域
    // 但设计支持多个区域（如 x86 可能有 below-4G 和 above-4G 两个区域）
    
    // 2. 一次性 mmap 所有区域
    let guest_memory = GuestMemoryMmap::from_ranges(&regions)?;
    //     ↑ 这里会遍历 regions，对每个区域调用 mmap()
    
    Ok((guest_memory, arch_memory_info))
}
```

**为什么设计成支持多个区域？**:
- x86_64 架构可能有多个内存区域（below-4G 和 above-4G，中间是 MMIO gap）
- LoongArch 只有一个连续区域（从 0x40000000 开始）
- 代码设计为通用架构，支持多种 CPU

---

**问题 2**: mmap 分配后，KVM 如何建立 GPA→HPA 映射？

**回答**: 分三步：

```
步骤 1: 用户空间 mmap 分配
─────────────────────────────────────────────────────────────────
// GuestMemoryMmap::from_ranges() 内部
for region in regions {
    // 1. 调用 mmap 分配用户空间内存
    let addr = mmap(
        NULL,           // 让内核选择地址
        region.size,    // 区域大小（如 2GB）
        PROT_READ | PROT_WRITE,
        MAP_PRIVATE | MAP_ANONYMOUS,
        -1,             // 匿名映射，不关联文件
        0
    );
    // addr = 0x7f0000000000 (示例，用户空间虚拟地址)
    
    // 2. 记录映射关系
    regions.push(MemoryRegion {
        guest_phys_addr: 0x40000000,  // GPA: Guest 物理地址
        userspace_addr: addr,          // HVA: Host 虚拟地址
        size: 0x80000000,              // 大小（2GB）
    });
}
```

```
步骤 2: 注册到 KVM（建立 GPA→HVA 映射）
─────────────────────────────────────────────────────────────────
// Vm::memory_init()
for region in &regions {
    let memory_region = kvm_userspace_memory_region {
        slot: slot_id,
        guest_phys_addr: region.guest_phys_addr,  // GPA = 0x40000000
        memory_size: region.size,                  // 大小 = 2GB
        userspace_addr: region.userspace_addr,     // HVA = 0x7f0000000000
        flags: 0,
    };
    
    // 通过 ioctl 告诉 KVM
    vm_fd.set_user_memory_region(memory_region)?;
    //     ↑ KVM_SET_USER_MEMORY_REGION
}

// KVM 内核空间（简化版）
int kvm_set_user_memory_region() {
    // 1. 获取 HVA
    unsigned long hva = mem->userspace_addr;  // 0x7f0000000000
    
    // 2. 通过 get_user_pages() 获取 HPA
    //    这会 pin 住用户空间页面，防止 swap
    struct page **pages = get_user_pages(hva, num_pages);
    
    // 3. 建立 GPA→HPA 映射表（memslot）
    slot->base_gfn = gpa_to_gfn(gpa);  // GPA → GFN（页帧号）
    slot->rmap = build_rmap(pages);    // GFN → HPA
    
    // 4. 如果使用 EPT/NPT，更新硬件页表
    update_ept_mapping(slot);
}
```

```
步骤 3: Guest 访问时的地址转换
─────────────────────────────────────────────────────────────────
Guest 执行：store $t0, 0x1000($zero)  # 写入 GVA = 0x1000
    ↓
【CPU MMU - Guest 页表】
GVA (0x1000) → GPA (0x40001000)
    ↓
【CPU EPT/NPT - KVM 页表】
GPA (0x40001000) → HPA (物理内存地址)
    ↓
【CPU MMU - Host 页表】
HPA → 访问真实 DRAM
    ↓
数据写入完成

关键：EPT/NPT 转换由 CPU 硬件自动完成，不需要 VM Exit！
```

---

**问题 3**: Guest 写入时的完整流程？

**回答**:

```
场景：Guest 内核执行 *(u32 *)0x90000000 = 0x12345678
      （写入 RAM，GVA = 0x90000000）

┌─────────────────────────────────────────────────────────────────┐
│ Guest (Non-root Mode)                                           │
│                                                                 │
│ store $t0, 0x90000000($zero)                                    │
│   ↓                                                             │
│ CPU MMU 查找 Guest 页表                                          │
│   ↓                                                             │
│ GVA 0x90000000 → GPA 0x50000000                                 │
│   ↓                                                             │
│ CPU EPT 查找 EPT 页表（由 KVM 管理）                               │
│   ↓                                                             │
│ GPA 0x50000000 → HPA 0x80000000 (示例)                          │
│   ↓                                                             │
│ 写入物理内存地址 0x80000000                                      │
│   ↓                                                             │
│ 完成！不会 VM Exit！                                             │
└─────────────────────────────────────────────────────────────────┘
```

**关键点**:
1. **GVA→GPA**: Guest 内核管理自己的页表，CPU MMU 自动转换
2. **GPA→HPA**: EPT/NPT 硬件自动转换，**不会 VM Exit**
3. **只有访问 MMIO 时才会 VM Exit**（因为 MMIO 不在 EPT 映射中）

---

#### 完整内存初始化流程

```rust
// 步骤 1: 计算内存布局（LoongArch 特定）
let (arch_memory_info, regions) = arch::loongarch64::arch_memory_regions(
    mem_size,      // 如 2GB
    initrd_size,
    None,
);
// regions = [(GuestAddress(0x40000000), 0x80000000)]
// 一个连续区域：GPA 0x40000000 开始，大小 2GB

// 步骤 2: 用户空间 mmap 分配
let guest_memory = GuestMemoryMmap::from_ranges(&regions)?;
// 内部调用：
//   mmap(NULL, 0x80000000, ...) → 返回 HVA（如 0x7f0000000000）
// 建立映射表：
//   region[0] = {
//     guest_phys_addr: 0x40000000,  // GPA
//     userspace_addr: 0x7f0000000000, // HVA
//     size: 0x80000000,              // 大小
//   }

// 步骤 3: 注册到 KVM
vm.memory_init(&guest_memory)?;
// 内部遍历所有区域：
//   for region in regions {
//     kvm_userspace_memory_region {
//       guest_phys_addr: 0x40000000,
//       memory_size: 0x80000000,
//       userspace_addr: 0x7f0000000000,
//     }
//     vm_fd.set_user_memory_region(region)?;
//   }

// 步骤 4: KVM 内核建立 GPA→HPA 映射
// kvm_set_user_memory_region() 内部：
//   hva = 0x7f0000000000
//   pages = get_user_pages(hva, num_pages)  // pin 住页面
//   slot->base_gfn = 0x40000  // GPA 0x40000000 → GFN 0x40000
//   slot->rmap = build_rmap(pages)  // GFN → HPA
//   update_ept_mapping(slot)  // 更新 EPT 页表
```

---

#### 内存虚拟化关键点总结

| 步骤 | 操作 | 位置 | 是否 VM Exit |
|------|------|------|-------------|
| mmap 分配 | 分配用户空间内存 | 用户空间 | ❌ 否 |
| 注册到 KVM | 建立 GPA→HVA 映射 | ioctl → 内核 | ❌ 否 |
| get_user_pages | pin 住用户页面 | 内核 | ❌ 否 |
| 更新 EPT | 建立 GPA→HPA 映射 | 内核 | ❌ 否 |
| Guest 访问 RAM | GVA→GPA→HPA | CPU 硬件 | ❌ 否 |
| Guest 访问 MMIO | 不在 EPT 中 | CPU 硬件 | ✅ 是 |

**关键理解**:
1. **mmap 分配的是用户空间虚拟地址 (HVA)**
2. **KVM 通过 `get_user_pages()` 获取物理页面 (HPA)**
3. **EPT/NPT 建立 GPA→HPA 的直接映射**
4. **Guest 访问 RAM 时，CPU 硬件自动完成转换，不会 VM Exit**
5. **只有访问 MMIO（不在 EPT 中）才会 VM Exit**

---

### 2. 中断虚拟化 (Interrupt Virtualization) - 深入详解

#### 你的疑问解答

**问题**: 中断虚拟化就是我实现的那部分，MMIO 注册 irqchip，自行调用 KVM INT ioctl？没有用物理 irqchip？

**回答**: **完全正确！** 你理解了核心要点。

**当前实现的中断路径**:
```
┌─────────────────────────────────────────────────────────────────┐
│  当前实现 (KVM_INTERRUPT + cpuintc)                              │
│                                                                 │
│  Virtio 设备完成 I/O                                             │
│      ↓                                                          │
│  mmio.signal_used_queue()                                       │
│      ↓                                                          │
│  intc.set_irq_state(irq=6, active=true)                         │
│      ↓                                                          │
│  KVM_INTERRUPT ioctl (signed_irq=+6)  ← 直接调用 KVM ioctl       │
│      ↓                                                          │
│  KVM 内核：kvm_queue_irq(vcpu, 6)                               │
│      ↓                                                          │
│  设置 vcpu->arch.pending_irqs |= (1 << 6)                       │
│      ↓                                                          │
│  Guest CPU 收到 HWI6 中断                                         │
│      ↓                                                          │
│  Guest: do_irq() → virtio_mmio_irqhandler()                     │
│                                                                 │
│  注意：没有使用物理 irqchip，也没有使用 PCHPIC/EIOINTC！         │
└─────────────────────────────────────────────────────────────────┘
```

**为什么不用物理 irqchip？**:
1. **旧版 LoongArch KVM 不支持**: irqfd/routing 链路不完整
2. **简化实现**: `KVM_INTERRUPT` 直接注入到 CPU INTC，更可靠
3. **性能考虑**: 少一层中断路由，更快

**代码对应**:
```rust
// src/devices/src/legacy/kvmloongarchirqchip.rs
impl IrqChipT for KvmLoongArchIrqChip {
    fn set_irq_state(
        &self,
        irq_line: Option<u32>,
        _interrupt_evt: Option<&EventFd>,
        active: bool,
    ) -> Result<(), DeviceError> {
        let irq = irq_line.unwrap();
        
        // 计算有符号中断号
        let signed_irq = if active {
            irq as i32       // 正数 = 断言
        } else {
            -(irq as i32)    // 负数 = 撤销
        };
        
        // 直接调用 KVM_INTERRUPT ioctl
        let interrupt = kvm_interrupt {
            irq: signed_irq as u32,
        };
        
        // 不使用 PCHPIC/EIOINTC，直接注入到 vCPU
        let ret = unsafe {
            ioctl_with_ref(&self.irq_vcpu_fd, KVM_INTERRUPT_LOONGARCH(), &interrupt)
        };
        
        debug!("KVM_INTERRUPT ok: irq={}, signed_irq={}, active={}",
               irq, signed_irq, active);
        
        Ok(())
    }
}
```

**对比：传统中断路径（未使用）**:
```
┌─────────────────────────────────────────────────────────────────┐
│  传统路径 (未使用)                                               │
│                                                                 │
│  Virtio 设备完成 I/O                                             │
│      ↓                                                          │
│  写入 eventfd                                                    │
│      ↓                                                          │
│  KVM irqfd 机制                                                  │
│      ↓                                                          │
│  KVM 内核：kvm_set_irq()                                        │
│      ↓                                                          │
│  路由到 PCHPIC (GSI routing)                                    │
│      ↓                                                          │
│  PCHPIC 触发中断                                                 │
│      ↓                                                          │
│  EIOINTC 路由                                                    │
│      ↓                                                          │
│  CPU INTC                                                        │
│      ↓                                                          │
│  Guest CPU 收到中断                                              │
│                                                                 │
│  问题：旧版 LoongArch KVM 缺少 kvm_set_routing_entry() 实现      │
│        所以这条链路走不通！                                      │
└─────────────────────────────────────────────────────────────────┘
```

---

#### MMIO 注册 irqchip 的流程

**问题**: MMIO 注册 irqchip 是怎么做的？

**回答**:

```rust
// 步骤 1: 创建 irqchip
// src/vmm/src/builder.rs
intc = Arc::new(Mutex::new(IrqChipDevice::new(Box::new(
    KvmLoongArchIrqChip::new(
        vm.fd(),
        vm_resources.vm_config().vcpu_count.unwrap() as u32,
    )
    .unwrap(),
))));

// 步骤 2: 注册 MMIO 串口设备
attach_legacy_devices(
    &vm,
    &mut mmio_device_manager,
    &mut kernel_cmdline,
    intc.clone(),  // ← 传递 irqchip
    serial_devices,
)?;

// 步骤 3: 在 register_mmio_serial 中使用 irqchip
// src/vmm/src/device_manager/kvm/mmio.rs
pub fn register_mmio_serial(
    &mut self,
    vm_fd: &VmFd,
    kernel_cmdline: &mut Cmdline,
    intc: IrqChip,  // ← 接收 irqchip
    serial: Arc<Mutex<Serial>>,
) -> Result<()> {
    // 创建 MMIO 传输层
    let mmio_device = MmioTransport::new(
        self.guest_memory.clone(),
        intc.clone(),  // ← 传递给 MmioTransport
        serial,
    )?;
    
    // 注册到 MMIO 总线
    let (mmio_base, irq) = self.register_mmio_device(
        vm_fd,
        mmio_device,
        VirtioType::Serial,
        "serial".to_string(),
    )?;
    
    // mmio_base = 0x0a001000 (串口 MMIO 基地址)
    // irq = 2 (中断线号)
    
    Ok(())
}
```

**关键点**:
1. **irqchip 在创建时传递给 MmioTransport**
2. **MmioTransport 包装 irqchip，实现中断注入**
3. **设备触发中断时，调用 `intc.set_irq_state()`**
4. **`set_irq_state()` 内部调用 `KVM_INTERRUPT` ioctl**

---

#### 中断虚拟化关键点总结

| 组件 | 作用 | 是否使用 |
|------|------|---------|
| KVM_INTERRUPT | 直接注入中断到 vCPU | ✅ 使用 |
| PCHPIC | PCI 主机中断控制器 | ❌ 创建但未使用 |
| EIOINTC | 扩展 I/O 中断控制器 | ❌ 创建但未使用 |
| IPI | 处理器间中断 | ❌ 创建但未使用 |
| irqfd | 事件 fd 触发中断 | ❌ 未使用 |
| GSI routing | 中断路由表 | ❌ 未使用 |

**为什么创建 PCHPIC/EIOINTC 但不使用？**:
- 为了平台兼容性（未来可能启用）
- FDT 中需要描述这些节点
- Guest 内核可以读取/写入这些寄存器（模拟）

---

### 3. 寄存器虚拟化 (Register Virtualization) - 深入详解

#### 你的疑问解答

**问题**: 寄存器虚拟化就是创建 vcpu 的时候把这些寄存器 cpucfg iocsr 配置为 mmio 还是？看起来也分配了对应地址存储。

**回答**: **不是 MMIO！** CPUCFG 和 IOCSR 是**CSR（控制状态寄存器）**，不是 MMIO。

**关键区别**:

| 特性 | MMIO | CSR (CPUCFG/IOCSR) |
|------|------|-------------------|
| 访问方式 | 加载/存储指令 | 专用 CSR 指令 |
| 地址空间 | Guest 物理地址 | CSR 地址空间 |
| VM Exit | 是（不在 EPT 中） | 是（特权指令） |
| 模拟位置 | 用户空间 VMM | 用户空间 VMM |
| 示例地址 | 0x0a001000 (串口) | 0x8 (CPUID) |

**访问方式对比**:
```rust
// MMIO 访问（普通加载/存储指令）
let val = *(0x0a001000 as *const u32);  // 读取串口寄存器
//     ↑ 这会触发 VM Exit（因为 MMIO 不在 EPT 中）

// CSR 访问（专用指令）
let val = cpucfg(0);  // 读取 CPUCFG0
//    ↑ 这也会触发 VM Exit（因为 CSR 虚拟化）
```

---

#### CPUCFG vs IOCSR

**CPUCFG (CPU Configuration Registers)**:
- **用途**: CPU 特性配置寄存器
- **数量**: CPUCFG0-CPUCFGn（通常 0-5）
- **访问指令**: `cpucfg rd, rj`（读取）
- **虚拟化方式**: KVM_SET_ONE_REG / KVM_GET_ONE_REG ioctl

**IOCSR (I/O Control and Status Registers)**:
- **用途**: I/O 控制和状态寄存器
- **数量**: 多个（0x8, 0x10, 0x20, 0x420, 0x1048 等）
- **访问指令**: `iocsrrd.d rd, rj`（读取双字）
- **虚拟化方式**: KVM_EXIT_IOCSR → 用户空间模拟

---

#### 寄存器虚拟化流程

```rust
// 步骤 1: 创建 vCPU 时配置寄存器
// src/vmm/src/builder.rs
vcpu.configure_loongarch64(
    vm.fd(),
    entry_addr,
    cmdline_addr,
    efi_system_table_addr
)?;

// 步骤 2: configure_loongarch64 内部
// src/vmm/src/linux/vstate.rs
pub fn configure_loongarch64(
    &mut self,
    _vm_fd: &VmFd,
    kernel_load_addr: GuestAddress,
    cmdline_addr: GuestAddress,
    efi_system_table_addr: GuestAddress,
) -> Result<()> {
    // 调用架构特定的寄存器配置
    arch::loongarch64::regs::setup_regs(
        &self.fd,
        kernel_load_addr.raw_value(),
        cmdline_addr.raw_value(),
        true,  // efi_boot
        efi_system_table_addr.raw_value(),
    )?;
    Ok(())
}

// 步骤 3: setup_regs 内部
// src/arch/src/loongarch64/linux/regs.rs
pub fn setup_regs(
    vcpu: &VcpuFd,
    boot_ip: u64,
    cmdline_addr: u64,
    efi_boot: bool,
    system_table: u64,
) -> Result<()> {
    // 1. 配置 CPUCFG（通过 KVM_SET_ONE_REG ioctl）
    setup_cpucfg(vcpu)?;
    
    // 2. 配置通用寄存器（通过 KVM_SET_REGS ioctl）
    let mut regs = vcpu.get_regs()?;
    regs.pc = boot_ip;
    regs.gpr[4] = u64::from(efi_boot);
    regs.gpr[5] = cmdline_addr;
    regs.gpr[6] = system_table;
    vcpu.set_regs(&regs)?;
    
    Ok(())
}

// 步骤 4: setup_cpucfg 内部
fn setup_cpucfg(vcpu: &VcpuFd) -> Result<()> {
    for index in 0..=5u64 {
        // 读取 host CPUCFG
        let host_value = read_host_cpucfg(index);
        
        // 过滤后写入 guest
        let guest_value = filter_cpucfg_for_kvm(index, host_value);
        
        // 通过 KVM_SET_ONE_REG ioctl 写入
        vcpu.set_one_reg(cpucfg_reg_id(index), &guest_value.to_le_bytes())?;
        //     ↑ KVM_SET_ONE_REG
    }
    Ok(())
}
```

---

#### IOCSR 模拟

**IOCSR 不是预先配置的**，而是在 Guest 访问时**动态模拟**：

```rust
// Guest 执行：iocsrrd.d $t0, 0x8  (读取 IOCSR_CPUID)
//     ↓
// VM Exit (KVM_EXIT_IOCSR)
//     ↓
// KVM_RUN 返回用户空间
//     ↓
// 检查退出原因
match exit_reason {
    VcpuExit::IocsrRead(addr, data) => {
        // 调用 IOCSR 处理函数
        process_iocsr_read(addr, data, &self.iocsr_state, self.id)?;
    }
}

// process_iocsr_read 内部
fn process_iocsr_read(addr: u64, data: &mut [u8], ...) -> IocsrReadResult {
    match addr {
        0x8 => {
            // IOCSR_CPUID
            let value = 0x818u64;  // 中断特性
            data.copy_from_slice(&value.to_le_bytes()[..data.len()]);
            debug!("LoongArch IOCSR read: addr=0x{:x}, len={}, value=0x{:x}",
                   addr, data.len(), value);
            IocsrReadResult::Value(value)
        }
        0x10 => {
            // IOCSR_VENDOR
            let value = 0u64;  // KVM Guest
            data.copy_from_slice(&value.to_le_bytes()[..data.len()]);
            IocsrReadResult::Value(value)
        }
        // ...
    }
}
```

**关键点**:
1. **IOCSR 没有预先分配存储地址**
2. **每次 Guest 访问时，VMM 动态返回模拟值**
3. **iocsr_state 用于保存可写的 IOCSR 寄存器状态**

---

#### 寄存器虚拟化关键点总结

| 寄存器类型 | 配置时机 | 存储位置 | 访问方式 |
|-----------|---------|---------|---------|
| CPUCFG | vCPU 创建时 | KVM 内核 | KVM_SET_ONE_REG |
| PC/GPR | vCPU 创建时 | KVM 内核 | KVM_SET_REGS |
| IOCSR (只读) | 访问时模拟 | 无（硬编码） | KVM_EXIT_IOCSR |
| IOCSR (可写) | 访问时模拟 | iocsr_state | KVM_EXIT_IOCSR |

---

### 4. IOCSR 首次执行详解

#### 日志：`LoongArch IOCSR read: addr=0x8` 是在干什么？

**回答**: 这是 Guest 内核启动时**读取 CPU 信息**，用于识别 CPU 型号和特性。

**Guest 内核代码**（简化版）:
```c
// Linux 内核 arch/loongarch/kernel/cpu-probe.c
void cpu_probe(void)
{
    // 读取 CPUID
    u32 cpucfg0 = iocsrrd_d(CSR_CPUID);
    //     ↑ 对应指令：iocsrrd.d $t0, 0x8
    //     ↑ 这会触发 VM Exit
    
    // 识别 CPU 型号
    if ((cpucfg0 & 0xffff) == 0xd010) {
        // Loongson-64bit (LA664)
        cpu_name = "Loongson-64bit";
    }
    
    // 读取厂商标识
    u64 vendor = iocsrrd_d(CSR_VENDOR);
    //     ↑ 对应指令：iocsrrd.d $t0, 0x10
    
    if (vendor == 0) {
        // KVM Guest
        pr_info("Running in KVM guest\n");
    }
}
```

**执行流程**:
```
Guest 内核启动
    ↓
cpu_probe()
    ↓
执行指令：iocsrrd.d $t0, 0x8
    ↓
VM Exit (KVM_EXIT_IOCSR)
    ↓
KVM_RUN 返回用户空间
    ↓
VMM 检查退出原因
    ↓
match VcpuExit::IocsrRead(addr=0x8, data)
    ↓
调用 process_iocsr_read(0x8, data)
    ↓
返回 0x818（中断特性）
    ↓
KVM_RUN 再次调用
    ↓
VM Entry
    ↓
Guest 继续执行，$t0 = 0x818
    ↓
打印日志：CPU0 revision is: 0014d010 (Loongson-64bit)
```

**为什么 Guest 要读取 IOCSR？**:
1. **识别 CPU 型号**: 不同 LoongArch CPU 有不同的特性
2. **检测虚拟化环境**: vendor=0 表示 KVM Guest
3. **配置内核行为**: 根据 CPU 特性启用/禁用某些功能

---

#### IOCSR 访问日志解析

```
[2026-03-24T06:37:10.373300Z DEBUG vmm::linux::vstate] 
    LoongArch IOCSR read: addr=0x8, len=4, value=0x818
    # Guest 读取 CPUID，VMM 返回 0x818（中断特性）
    
[2026-03-24T06:37:10.373371Z DEBUG vmm::linux::vstate] 
    LoongArch IOCSR read: addr=0x10, len=8, value=0x0
    # Guest 读取 VENDOR，VMM 返回 0（KVM Guest）
    
[2026-03-24T06:37:10.373378Z DEBUG vmm::linux::vstate] 
    LoongArch IOCSR read: addr=0x20, len=8, value=0x0
    # Guest 读取 FEATURES，VMM 返回 0（KVM Guest）
```

**Guest 内核输出**:
```
[    0.000000][    T0] 64-bit Loongson Processor probed (LA664 Core)
[    0.000000][    T0] CPU0 revision is: 0014d010 (Loongson-64bit)
[    0.000000][    T0] efi: EFI v2.1 by libkrun
```

Guest 内核根据 IOCSR 读取的值，正确识别了 CPU 型号和虚拟化环境。

---

### 5. 完整执行流程示例：VirtioFS 读文件

```
时间线：T0 - Guest 应用发起读请求
─────────────────────────────────────────────────────────────────
Guest (Non-root)
open("/etc/passwd")
    ↓ VFS
virtiofs_init_inode()
    ↓ 驱动
virtio_fs_enqueue_req()
    ↓
1. 分配 descriptor (Guest RAM) ← 不会 VM Exit
2. 填充请求数据 (Guest RAM) ← 不会 VM Exit
3. 更新 avail ring (Guest RAM) ← 不会 VM Exit
4. writel(0, MMIO_NOTIFY) ← 会 VM Exit!
                            ↓ VM Exit (MMIO Write)
时间线：T1 - KVM 处理 MMIO 退出
─────────────────────────────────────────────────────────────────
KVM (Root)
kvm_vcpu_run()
    ↓
检测到 KVM_EXIT_MMIO
    ↓
KVM_RUN 返回用户空间
exit_reason = KVM_EXIT_MMIO
mmio.addr = 0x0a005034
mmio.data = 0x0
                            ↓ ioctl return
时间线：T2 - VMM 模拟设备
─────────────────────────────────────────────────────────────────
User Space (libkrun)
build_microvm() 主循环
    ↓
match exit_reason {
  KVM_EXIT_MMIO => {
    mmio_bus.write(addr, data)
      ↓
    MmioTransport::write()
      ↓
    VirtioFs::notify(queue=0)
      ↓
    唤醒 worker 线程
  }
}

Worker 线程：
process_queue()
    ↓
从 virtqueue 读取请求 (通过 SHM/Guest RAM)
    ↓
解析 FUSE 请求：FUSE_LOOKUP("/etc/passwd")
    ↓
访问 Host 文件系统：File::open("/host/rootfs/etc/passwd")
    ↓
将结果写入 used ring (Guest RAM)
    ↓
intc.set_irq_state(irq=6, active=true)
      ↓
KVM_INTERRUPT ioctl ← 注入中断
                            ↓ KVM_RUN again
时间线：T3 - KVM 注入中断
─────────────────────────────────────────────────────────────────
KVM (Root)
kvm_vm_ioctl_interrupt()
    ↓
kvm_queue_irq(vcpu, 6)
    ↓
设置 vcpu->arch.pending_irqs |= (1 << 6)
    ↓
VM Entry
                            ↓ VM Entry
时间线：T4 - Guest 处理中断
─────────────────────────────────────────────────────────────────
Guest (Non-root)
CPU 检测到 pending IRQ6
    ↓
do_irq(regs)
    ↓
handle_irq(6, regs)
    ↓
generic_handle_domain_irq(6)
    ↓
virtio_mmio_irqhandler(6)
    ↓
readl(MMIO_INTERRUPT_STATUS) ← 会 VM Exit!
                            ↓ VM Exit (MMIO Read)
时间线：T5 - VMM 模拟 MMIO 读
─────────────────────────────────────────────────────────────────
User Space (libkrun)
KVM_EXIT_MMIO
    ↓
MmioTransport::read()
    ↓
VirtioFs::read_interrupt_status()
    ↓
返回 0x1 (VRING 中断挂起)
    ↓
KVM_RUN 再次调用
                            ↓ VM Entry
时间线：T6 - Guest 继续中断处理
─────────────────────────────────────────────────────────────────
Guest (Non-root)
virtio_mmio_irqhandler() 继续
    ↓
vring_interrupt()
    ↓
回调设备驱动：virtio_fs_request_done()
    ↓
唤醒等待的进程：wake_up(&req->waitq)
    ↓
writel(0x1, MMIO_INTERRUPT_ACK) ← 会 VM Exit!
                            ↓ VM Exit (MMIO Write)
时间线：T7 - VMM 清除中断状态
─────────────────────────────────────────────────────────────────
User Space (libkrun)
KVM_EXIT_MMIO
    ↓
MmioTransport::write()
    ↓
VirtioFs::write_interrupt_ack()
    ↓
清除中断状态：status &= ~0x1
    ↓
intc.set_irq_state(irq=6, active=false)
      ↓
KVM_INTERRUPT ioctl (signed_irq=-6) ← 撤销中断
                            ↓ KVM_RUN again
时间线：T8 - Guest 返回应用
─────────────────────────────────────────────────────────────────
Guest (Non-root)
从中断返回
    ↓
virtio_fs_request_done() 继续
    ↓
复制数据到用户空间
    ↓
返回到应用：return fd
```

**VM Exit 统计**:
- T0→T1: MMIO Write (通知队列)
- T4→T5: MMIO Read (读取中断状态)
- T6→T7: MMIO Write (写入 ACK)

**不会 VM Exit 的操作**:
- Guest RAM 访问 (descriptor/vring 操作)
- CPU 寄存器操作
- 中断处理函数调用
- 进程唤醒

---

*最后更新：2026 年 3 月 24 日*

---

## 附录 D: 日志详解 - 从日志看启动流程

本节结合实际运行日志，详细解释每条日志的含义和对应的执行流程。

### 日志时间线总览

```
时间戳                      阶段                    说明
─────────────────────────────────────────────────────────────────────────
06:37:09.496016 - 06:37:09.743989   内核加载          PE/GZ 解压、CPUCFG 配置
06:37:10.371416 - 06:37:10.372052   FDT 创建          设备树构建
06:37:10.373300 - 06:37:10.660272   vCPU 启动         IOCSR 模拟、首次执行
06:37:11.276386 - 06:37:11.280931   Console 初始化    Virtio 控制台设置
06:37:11.433402 - 06:37:11.488544   VirtioFS 初始化   FUSE_INIT、文件查找
06:37:11.488544 - ...              根文件系统挂载    读取/init、库文件
```

---

### 阶段 1: 内核加载 (06:37:09.496016)

#### 日志 1: 发现 GZIP 头部
```
[2026-03-24T06:37:09.496016Z DEBUG vmm::builder] 
    Found GZIP header on PE file at: 0x8450
```

**含义**: 在 PE 文件的 `0x8450` 偏移处发现 GZIP 压缩头

**对应代码**:
```rust
// src/vmm/src/builder.rs
let data: Vec<u8> = std::fs::read(external_kernel.path)?;
if let Some(magic) = data.windows(3).position(|window| window == [0x1f, 0x8b, 0x8]) {
    debug!("Found GZIP header on PE file at: 0x{magic:x}");
    // magic = 0x8450
}
```

**说明**: 
- `0x1f 0x8b 0x08` 是 GZIP 格式的魔数
- 内核是 PE/GZ 格式（PE 头部 + GZIP 压缩的内核镜像）

---

#### 日志 2: 计算加载地址
```
[2026-03-24T06:37:09.730475Z DEBUG vmm::builder] 
    loongarch pegz image_load_addr=0x40200000, entry_addr=0x41660000
```

**含义**: 
- `image_load_addr=0x40200000`: 内核镜像加载到 Guest 物理地址 `0x40200000`
- `entry_addr=0x41660000`: 内核入口点在 `0x41660000`

**计算过程**:
```rust
// 从 PE 头部解析
load_offset = 0x200000;  // 从 PE 头部读取
kernel_entry = 0x9000000000216600;  // 从 PE 头部读取

// 计算实际地址
image_load_addr = DRAM_MEM_START + load_offset
                = 0x40000000 + 0x200000
                = 0x40200000

entry_offset = kernel_entry - LOONGARCH_VMLINUX_LOAD_ADDRESS
             = 0x9000000000216600 - 0x9000000000200000
             = 0x16600

entry_addr = image_load_addr + entry_offset
           = 0x40200000 + 0x16600
           = 0x40216600
```

**注意**: 实际入口地址是 `0x41660000`，说明内核在解压后重定位了。

---

#### 日志 3: CPUCFG 配置
```
[2026-03-24T06:37:09.743935Z DEBUG arch::loongarch64::linux::regs] 
    loongarch set cpucfg0: host=0x14d010, guest=0x14d010
[2026-03-24T06:37:09.743945Z DEBUG arch::loongarch64::linux::regs] 
    loongarch set cpucfg1: host=0x7f2f2fe, guest=0x3f2f2fe
[2026-03-24T06:37:09.743950Z DEBUG arch::loongarch64::linux::regs] 
    loongarch set cpucfg2: host=0x7f7ccfcf, guest=0x60c0cf
[2026-03-24T06:37:09.743954Z DEBUG arch::loongarch64::linux::regs] 
    loongarch set cpucfg3: host=0xcefcff, guest=0xfcff
[2026-03-24T06:37:09.743959Z DEBUG arch::loongarch64::linux::regs] 
    loongarch set cpucfg4: host=0x5f5e100, guest=0x5f5e100
[2026-03-24T06:37:09.743963Z DEBUG arch::loongarch64::linux::regs] 
    loongarch set cpucfg5: host=0x10001, guest=0x10001
```

**含义**: 配置 vCPU 的 CPUCFG 寄存器（CPU 特性寄存器）

**详细解析**:

| 寄存器 | Host 值 | Guest 值 | 说明 |
|--------|---------|----------|------|
| CPUCFG0 | `0x14d010` | `0x14d010` | CPU 型号：LA664 |
| CPUCFG1 | `0x7f2f2fe` | `0x3f2f2fe` | 过滤后的特性位 |
| CPUCFG2 | `0x7f7ccfcf` | `0x60c0cf` | LSX/LASX 向量扩展 |
| CPUCFG3 | `0xcefcff` | `0xfcff` | 其他特性 |
| CPUCFG4 | `0x5f5e100` | `0x5f5e100` | 平台特性 |
| CPUCFG5 | `0x10001` | `0x10001` | 扩展特性 |

**为什么 Host 和 Guest 值不同？**:
```rust
// src/arch/src/loongarch64/linux/regs.rs
fn filter_cpucfg_for_kvm(index: u64, host_value: u64) -> u64 {
    match index {
        0 => host_value & 0xffff_ffff,  // CPUCFG0: 全保留
        1 => host_value & CPUCFG1_KVM_MASK,  // CPUCFG1: 过滤某些位
        2 => {
            // CPUCFG2: 根据 Host 支持情况暴露 LSX/LASX
            let mut mask = CPUCFG2_FP | CPUCFG2_FPSP | ...;
            if host_value & CPUCFG2_LSX != 0 {
                mask |= CPUCFG2_LSX;  // Host 支持 LSX，暴露给 Guest
            }
            if host_value & CPUCFG2_LASX != 0 {
                mask |= CPUCFG2_LASX;  // Host 支持 LASX，暴露给 Guest
            }
            host_value & mask
        }
        // ...
    }
}
```

**目的**: 
- 隐藏某些 Host 特有的特性
- 保证 Guest 看到的 CPU 特性是安全和一致的

---

#### 日志 4: 寄存器配置完成
```
[2026-03-24T06:37:09.743983Z DEBUG arch::loongarch64::linux::regs] 
    loongarch setup_regs: pc=0x41660000, a0=1, a1=0xbffe8000, a2=0xbffec000
```

**含义**: vCPU 寄存器配置完成

**寄存器值**:
| 寄存器 | 值 | 说明 |
|--------|-----|------|
| `pc` | `0x41660000` | 内核入口地址 |
| `gpr[4]` (a0) | `1` | EFI 启动标志（1=EFI） |
| `gpr[5]` (a1) | `0xbffe8000` | 命令行地址 |
| `gpr[6]` (a2) | `0xbffec000` | EFI System Table 地址 |

**对应代码**:
```rust
// src/arch/src/loongarch64/linux/regs.rs
pub fn setup_regs(vcpu: &VcpuFd, boot_ip: u64, cmdline_addr: u64, 
                  efi_boot: bool, system_table: u64) -> Result<()> {
    let mut regs = vcpu.get_regs()?;
    regs.pc = boot_ip;              // 0x41660000
    regs.gpr[4] = u64::from(efi_boot);  // 1
    regs.gpr[5] = cmdline_addr;         // 0xbffe8000
    regs.gpr[6] = system_table;         // 0xbffec000
    vcpu.set_regs(&regs)?;
    
    debug!("loongarch setup_regs: pc=0x{:x}, a0={}, a1=0x{:x}, a2=0x{:x}",
           regs.pc, regs.gpr[4], regs.gpr[5], regs.gpr[6]);
}
```

---

### 阶段 2: FDT 创建 (06:37:10.371416)

#### 日志 5: 命令行配置
```
[2026-03-24T06:37:10.371416Z DEBUG vmm::builder] 
    loongarch cmdline: addr=0xbffe8000, 
    cmdline=Cmdline { line: "console=ttyS0 root=/dev/root rootfstype=virtiofs rw init=/init earlycon=uart8250,mmio,0x0a001000", capacity: 2048 }
```

**含义**: 内核命令行已配置

**命令行参数解析**:
| 参数 | 值 | 说明 |
|------|-----|------|
| `console` | `ttyS0` | 主控制台设备 |
| `root` | `/dev/root` | 根文件系统设备 |
| `rootfstype` | `virtiofs` | 根文件系统类型 |
| `rw` | - | 读写挂载 |
| `init` | `/init` | init 进程路径 |
| `earlycon` | `uart8250,mmio,0x0a001000` | 早期串口控制台 |

**earlycon 参数详解**:
- `uart8250`: 使用 8250 UART 驱动
- `mmio`: MMIO 访问方式
- `0x0a001000`: 串口寄存器基地址

---

#### 日志 6: 设备树节点创建
```
[2026-03-24T06:37:10.372011Z DEBUG devices::fdt::loongarch64] 
    loongarch serial node: addr=0xa001000, len=0x1000, irq=2, clock-frequency=3686400
[2026-03-24T06:37:10.372018Z DEBUG devices::fdt::loongarch64] 
    loongarch virtio node: addr=0xa002000, irq=3
[2026-03-24T06:37:10.372023Z DEBUG devices::fdt::loongarch64] 
    loongarch virtio node: addr=0xa003000, irq=4
[2026-03-24T06:37:10.372028Z DEBUG devices::fdt::loongarch64] 
    loongarch virtio node: addr=0xa004000, irq=5
[2026-03-24T06:37:10.372033Z DEBUG devices::fdt::loongarch64] 
    loongarch virtio node: addr=0xa005000, irq=6
[2026-03-24T06:37:10.372037Z DEBUG devices::fdt::loongarch64] 
    loongarch virtio node: addr=0xa006000, irq=7
```

**含义**: FDT 设备树节点创建完成

**设备地址和中断映射**:
| 设备 | MMIO 地址 | IRQ | 说明 |
|------|----------|-----|------|
| Serial | `0x0a001000` | 2 | 串口控制台 |
| Virtio Balloon | `0x0a002000` | 3 | 内存气球 |
| Virtio Rng | `0x0a003000` | 4 | 随机数生成器 |
| Virtio Console | `0x0a004000` | 5 | 虚拟控制台 |
| Virtio FS | `0x0a005000` | 6 | 文件系统 |
| Virtio Vsock | `0x0a006000` | 7 | VM 间通信 |

**对应的 FDT 节点**:
```dts
/ {
    serial@a001000 {
        compatible = "ns16550a";
        reg = <0x0 0xa001000 0x0 0x1000>;
        clock-frequency = <3686400>;
        interrupts = <2>;
    };
    
    virtio_mmio@a002000 {
        compatible = "virtio,mmio";
        reg = <0x0 0xa002000 0x0 0x200>;
        interrupts = <3>;
    };
    
    // ... 其他设备
};
```

---

#### 日志 7: FDT 写入完成
```
[2026-03-24T06:37:10.372045Z DEBUG devices::fdt::loongarch64] 
    loongarch fdt written: addr=0xbfff0000, size=0x73f
```

**含义**: FDT 设备树已写入 Guest 内存

**参数**:
- `addr=0xbfff0000`: FDT 在 Guest 物理内存中的地址
- `size=0x73f`: FDT 大小（1855 字节）

**内存布局**:
```
0xbfff0000: ┌─────────────────┐
           │   FDT Header    │
           ├─────────────────┤
           │  Memory Map     │
           ├─────────────────┤
           │  Structure      │
           ├─────────────────┤
           │  Strings        │
           └─────────────────┘
0xbfff073f: (结束)
```

---

#### 日志 8: EFI System Table 创建
```
[2026-03-24T06:37:10.372052Z DEBUG arch::loongarch64::linux::efi] 
    loongarch efi handoff: systab=0xbffec000, config=0xbffec100, vendor=0xbffec200, fdt=0xbfff0000
```

**含义**: EFI System Table 创建完成

**地址布局**:
```
0xbffec000: EFI System Table Header
0xbffec100: EFI Config Table (包含 FDT GUID)
0xbffec200: Vendor String "libkrun"
0xbfff0000: FDT 设备树
```

**EFI Config Table 内容**:
```c
typedef struct {
    efi_guid_t guid;
    u64 table;
} efi_config_table_t;

// guid = DEVICE_TREE_GUID (0xb1b621d5-f19c-41a5-830b-d9152c69aae0)
// table = 0xbfff0000 (FDT 地址)
```

**内核如何使用**:
```c
// Linux 内核 arch/loongarch/kernel/efi.c
void __init efi_init(void)
{
    systab = (efi_system_table_t *)boot_params->efi_system_table;
    // systab = 0xbffec000
    
    for (i = 0; i < systab->nr_tables; i++) {
        if (guid_equal(&systab->tables[i].guid, &DEVICE_TREE_GUID)) {
            fdt_addr = systab->tables[i].table;
            // fdt_addr = 0xbfff0000
            break;
        }
    }
    
    early_init_dt_scan(fdt_addr);  // 解析 FDT
}
```

---

### 阶段 3: vCPU 启动与首次执行 (06:37:10.373300)

#### 日志 9: IOCSR 读取 - CPUID
```
[2026-03-24T06:37:10.373300Z DEBUG vmm::linux::vstate] 
    LoongArch IOCSR read: addr=0x8, len=4, value=0x818
```

**含义**: Guest 内核读取 IOCSR_CPUID 寄存器

**详细解析**:
- `addr=0x8`: IOCSR_CPUID 寄存器偏移
- `len=4`: 读取 4 字节（32 位）
- `value=0x818`: 返回值

**0x818 的位含义**:
```
0x818 = 0b1000_0001_1000
         ^^^^ ^^^^ ^^^^
          │    │    └─ 位 [3:0]: 保留
          │    └────── 位 [7:4]: IPI 支持 (0x1 = 支持)
          └─────────── 位 [11:8]: 定时器支持 (0x8 = 支持)
```

**对应代码**:
```rust
// src/vmm/src/linux/vstate.rs
fn handle_iocsr_read(&mut self, addr: u64, data: &mut [u8]) -> Result<()> {
    match addr {
        0x8 => {  // LOONGARCH_IOCSR_CPUID
            let value = 0x818u64;  // 中断特性
            data.copy_from_slice(&value.to_le_bytes()[..data.len()]);
            debug!("LoongArch IOCSR read: addr=0x{:x}, len={}, value=0x{:x}",
                   addr, data.len(), value);
        }
        // ...
    }
}
```

---

#### 日志 10: IOCSR 读取 - VENDOR
```
[2026-03-24T06:37:10.373371Z DEBUG vmm::linux::vstate] 
    LoongArch IOCSR read: addr=0x10, len=8, value=0x0
```

**含义**: Guest 内核读取 IOCSR_VENDOR 寄存器

**详细解析**:
- `addr=0x10`: IOCSR_VENDOR 寄存器偏移
- `len=8`: 读取 8 字节（64 位）
- `value=0x0`: 返回值（0 表示 KVM Guest）

**为什么返回 0？**:
- 物理 Loongson CPU 返回 "Loongson" 字符串的编码值
- KVM Guest 返回 0，表示虚拟化环境

---

#### 日志 11: IOCSR 读取 - FEATURES
```
[2026-03-24T06:37:10.373378Z DEBUG vmm::linux::vstate] 
    LoongArch IOCSR read: addr=0x20, len=8, value=0x0
```

**含义**: Guest 内核读取 IOCSR_FEATURES 寄存器

**详细解析**:
- `addr=0x20`: IOCSR_FEATURES 寄存器偏移
- `len=8`: 读取 8 字节（64 位）
- `value=0x0`: 返回值（KVM Guest 特性标识）

---

#### 日志 12: 中断撤销
```
[2026-03-24T06:37:10.383734Z DEBUG devices::legacy::kvmloongarchirqchip] 
    KVM_INTERRUPT ok: irq=2, signed_irq=-2, active=false
```

**含义**: 撤销 IRQ 2（串口中断）

**详细解析**:
- `irq=2`: 中断线号（对应 HWI0）
- `signed_irq=-2`: 有符号中断号（负数=撤销）
- `active=false`: 中断状态（false=撤销）

**为什么启动时撤销中断？**:
- 初始化时清除所有 pending 中断
- 确保启动时没有挂起的中断

---

### 阶段 4: VirtioFS 初始化 (06:37:11.433402)

#### 日志 13: VirtioFS Worker 启动
```
[2026-03-24T06:37:11.433402Z DEBUG devices::virtio::fs::worker] 
    Fs: queue event: 1
```

**含义**: VirtioFS Worker 线程被唤醒，处理队列事件

**触发原因**:
1. Guest 发送 FUSE 请求到 virtqueue
2. 写入 MMIO_NOTIFY 寄存器
3. VM Exit → VMM 唤醒 Worker 线程

---

#### 日志 14: FUSE_INIT 请求
```
[2026-03-24T06:37:11.433429Z DEBUG devices::virtio::fs::server] 
    opcode: 26
[2026-03-24T06:37:11.433438Z DEBUG devices::virtio::fs::server] 
    FUSE_INIT request: major=7, minor=40, flags=0x7bfffffb, flags2=0xfd
```

**含义**: Guest 发送 FUSE_INIT 初始化请求

**详细解析**:
- `opcode: 26`: FUSE_INIT 操作码
- `major=7, minor=40`: FUSE 协议版本 7.40
- `flags=0x7bfffffb`: 客户端支持的特性标志
- `flags2=0xfd`: 扩展特性标志

**FUSE_INIT 处理流程**:
```rust
// src/devices/src/virtio/fs/server.rs
fn fuse_init(&self, req: &Request) -> Result<Writer> {
    let arg = req.arg::<FuseInitIn>()?;
    
    debug!("FUSE_INIT request: major={}, minor={}, flags={:#x}, flags2={:#x}",
           arg.major, arg.minor, arg.flags, arg.flags2);
    
    // 响应 FUSE_INIT
    let out = FuseInitOut {
        major: 7,
        minor: 40,
        max_pages: 64,
        // ...
    };
    
    Ok(writer)
}
```

---

#### 日志 15: Passthrough 初始化
```
[2026-03-24T06:37:11.433469Z DEBUG devices::virtio::fs::linux::passthrough] 
    virtiofs passthrough init: root_dir=./rootfs_debian_unstable
```

**含义**: Passthrough 文件系统初始化

**详细解析**:
- `root_dir=./rootfs_debian_unstable`: Host 共享目录路径

**对应代码**:
```rust
// src/devices/src/virtio/fs/linux/passthrough.rs
pub fn init(&self, root_dir: &str) -> io::Result<()> {
    debug!("virtiofs passthrough init: root_dir={}", root_dir);
    
    // 打开共享目录
    let dir = File::open(root_dir)?;
    self.root_inode = Some(Arc::new(InodeData {
        inode: ROOT_INODE,
        file: dir,
        // ...
    }));
    
    Ok(())
}
```

---

#### 日志 16: Passthrough 初始化完成
```
[2026-03-24T06:37:11.434017Z DEBUG devices::virtio::fs::linux::passthrough] 
    virtiofs passthrough init ok: root_dir=./rootfs_debian_unstable, ino=6821155, dev=71, mnt_id=600
```

**含义**: Passthrough 文件系统初始化成功

**详细解析**:
- `ino=6821155`: 根目录的 inode 号
- `dev=71`: 设备号
- `mnt_id=600`: 挂载 ID

---

#### 日志 17: FUSE_INIT 响应
```
[2026-03-24T06:37:11.434029Z DEBUG devices::virtio::fs::server] 
    FUSE_INIT ok: want=0x8006000, enabled=0x5844f829, max_pages=64
```

**含义**: FUSE_INIT 请求处理完成

**详细解析**:
- `want=0x8006000`: Guest 期望的特性
- `enabled=0x5844f829`: 实际启用的特性
- `max_pages=64`: 最大传输页数（64 × 4KB = 256KB）

---

### 阶段 5: 文件系统查找 (06:37:11.437269)

#### 日志 18: 查找目录
```
[2026-03-24T06:37:11.437269Z DEBUG devices::virtio::fs::linux::passthrough] 
    do_lookup: "dev"
[2026-03-24T06:37:11.437293Z DEBUG devices::virtio::fs::linux::passthrough] 
    do_lookup: dev, inode: 3
```

**含义**: Guest 查找 `/dev` 目录

**详细解析**:
- `do_lookup: "dev"`: 查找名为 "dev" 的目录
- `inode: 3`: 找到的 inode 号

**对应代码**:
```rust
// src/devices/src/virtio/fs/linux/passthrough.rs
fn do_lookup(&self, name: &str) -> io::Result<Inode> {
    debug!("do_lookup: {:?}", name);
    
    // 在父目录中查找子项
    let entry = self.parent_dir.lookup(name)?;
    
    let inode = entry.inode();
    debug!("do_lookup: {}, inode: {}", name, inode);
    
    Ok(inode)
}
```

**Host 文件系统对应**:
```
./rootfs_debian_unstable/
├── dev/        ← inode: 3
├── etc/        ← inode: 4
├── init        ← inode: 5
├── bin/        ← inode: 6
└── ...
```

---

#### 日志 19: 查找 /init
```
[2026-03-24T06:37:11.444254Z DEBUG devices::virtio::fs::linux::passthrough] 
    do_lookup: "init"
[2026-03-24T06:37:11.444270Z DEBUG devices::virtio::fs::linux::passthrough] 
    do_lookup: init, inode: 5
```

**含义**: Guest 查找 `/init` 文件（init 进程）

**详细解析**:
- `inode: 5`: `/init` 文件的 inode 号

---

#### 日志 20: 打开 /init
```
[2026-03-24T06:37:11.445384Z DEBUG devices::virtio::fs::linux::passthrough] 
    do_open: 5
```

**含义**: Guest 打开 `/init` 文件

**详细解析**:
- `do_open: 5`: 打开 inode 5（即 `/init`）

**对应代码**:
```rust
fn do_open(&self, inode: Inode, flags: u32) -> io::Result<Handle> {
    debug!("do_open: {}", inode);
    
    // 打开文件
    let file = self.open_file(inode, flags)?;
    
    // 创建 handle
    let handle = Handle::new(file, inode);
    
    Ok(handle)
}
```

---

#### 日志 21: 读取 /init
```
[2026-03-24T06:37:11.447120Z DEBUG devices::virtio::fs::linux::passthrough] 
    read: 5
```

**含义**: Guest 读取 `/init` 文件内容

**详细解析**:
- `read: 5`: 读取 inode 5（即 `/init`）
- **注意**: 这里的 `5` 是 inode 号，**不是**读取的字节数！

**对应代码**:
```rust
// src/devices/src/virtio/fs/linux/passthrough.rs
fn read(&self, inode: Inode, handle: Handle, size: u32, offset: u64) 
    -> io::Result<Vec<u8>> {
    debug!("read: {:?}", inode);  // 打印 inode 号
    
    // 找到对应的文件
    let file = self.get_file(handle)?;
    
    // 读取文件内容
    let mut buffer = vec![0u8; size];
    file.read_at(&mut buffer, offset)?;
    
    Ok(buffer)
}
```

---

#### 日志 22: 查找动态链接库
```
[2026-03-24T06:37:11.453682Z DEBUG devices::virtio::fs::linux::passthrough] 
    do_lookup: "dash"
[2026-03-24T06:37:11.454058Z DEBUG devices::virtio::fs::linux::passthrough] 
    do_lookup: dash, inode: 10
[2026-03-24T06:37:11.457406Z DEBUG devices::virtio::fs::linux::passthrough] 
    do_open: 10
[2026-03-24T06:37:11.458972Z DEBUG devices::virtio::fs::linux::passthrough] 
    read: 10
```

**含义**: Guest 查找并打开 `/bin/dash`（shell 程序）

**详细解析**:
1. `do_lookup: "dash"`: 查找 dash
2. `inode: 10`: 找到 inode 10
3. `do_open: 10`: 打开 inode 10
4. `read: 10`: 读取 inode 10 的内容

**执行流程**:
```
Guest 执行 /init
    ↓
/init 是 ELF 可执行文件
    ↓
内核解析 ELF 头部
    ↓
发现需要动态链接器：/lib64/ld-linux-loongarch-lp64d.so.1
    ↓
查找并打开动态链接器
    ↓
动态链接器加载 /bin/dash
    ↓
读取 dash 文件内容
```

---

#### 日志 23: 查找共享库
```
[2026-03-24T06:37:11.485848Z DEBUG devices::virtio::fs::linux::passthrough] 
    do_lookup: "libc.so.6"
[2026-03-24T06:37:11.485863Z DEBUG devices::virtio::fs::linux::passthrough] 
    do_lookup: libc.so.6, inode: 18
[2026-03-24T06:37:11.486960Z DEBUG devices::virtio::fs::linux::passthrough] 
    do_open: 18
[2026-03-24T06:37:11.488544Z DEBUG devices::virtio::fs::linux::passthrough] 
    read: 18
```

**含义**: Guest 查找并打开 `libc.so.6`（C 标准库）

**详细解析**:
- `inode: 18`: `libc.so.6` 的 inode 号
- 这是动态链接器加载共享库的过程

---

### 日志与 VM Exit 对应关系

| 日志 | VM Exit 类型 | 说明 |
|------|-------------|------|
| IOCSR read | `KVM_EXIT_IOCSR` | Guest 读取 IOCSR 寄存器 |
| do_lookup | 无 | 纯软件操作，不触发 VM Exit |
| do_open | 无 | 纯软件操作，不触发 VM Exit |
| read | 无 | 纯软件操作，不触发 VM Exit |
| MMIO 访问 | `KVM_EXIT_MMIO` | 访问 Virtio 设备寄存器 |
| KVM_INTERRUPT | 无 | VMM 主动注入中断 |

**关键点**:
- **文件操作**（lookup/open/read）在 Host 用户空间执行，**不会触发 VM Exit**
- **MMIO 访问**（通知队列、读取中断状态）会触发 VM Exit
- **中断注入**由 VMM 主动触发，**不会触发 VM Exit**

---

### 完整日志时间线

```
时间 (相对)    日志内容                              阶段
─────────────────────────────────────────────────────────────────────
T+0.000s      Found GZIP header on PE file          内核加载
T+0.234s      loongarch pegz image_load_addr        地址计算
T+0.247s      loongarch set cpucfg0-5               CPUCFG 配置
T+0.248s      loongarch setup_regs                  寄存器配置
T+0.875s      loongarch cmdline                     命令行配置
T+0.876s      loongarch serial/virtio node          FDT 创建
T+0.876s      loongarch fdt written                 FDT 完成
T+0.876s      loongarch efi handoff                 EFI 创建
T+0.877s      LoongArch IOCSR read: addr=0x8        首次执行
T+0.877s      LoongArch IOCSR read: addr=0x10       首次执行
T+0.877s      LoongArch IOCSR read: addr=0x20       首次执行
T+0.888s      KVM_INTERRUPT ok: irq=2               中断撤销
T+1.780s      Fs: queue event: 1                    VirtioFS 唤醒
T+1.780s      opcode: 26                            FUSE_INIT
T+1.780s      virtiofs passthrough init             初始化
T+1.781s      FUSE_INIT ok                          初始化完成
T+1.784s      do_lookup: "dev"                      查找目录
T+1.791s      do_lookup: "init"                     查找 init
T+1.792s      do_open: 5                            打开 init
T+1.794s      read: 5                               读取 init
T+1.804s      do_lookup: "dash"                     查找 shell
T+1.833s      read: 18                              读取 libc.so.6
...           ...                                   继续执行
```

---

*最后更新：2026 年 3 月 24 日*
