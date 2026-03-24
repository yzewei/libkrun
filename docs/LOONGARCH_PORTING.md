# LoongArch 适配说明

本文档按“总分结构”整理当前 libkrun 的 LoongArch 适配状态。内容以当前代码为准，重点回答三个问题：

1. libkrun 整体是怎样组织的
2. LoongArch 相关代码分别落在哪些文件里
3. LoongArch 是按什么思路一步步适配出来的

## 1. libkrun 总体结构

### 1.1 分层视角

从代码组织上看，libkrun 可以粗分为 3 层：

- `src/arch`
  负责架构级内存布局、启动协议、寄存器配置、FDT/EFI 等“平台基础约束”
- `src/devices`
  负责设备模型，包括 legacy 设备、virtio 设备、FDT 描述和 irqchip 抽象
- `src/vmm`
  负责把内核、内存、vCPU、设备、irqchip 串成一个完整 VM，并驱动 KVM 运行

对应的主线关系是：

1. `builder.rs` 读取 VM 配置并创建 KVM VM
2. `arch` 层提供本架构的内存布局和启动约定
3. `devices` 层创建串口、virtio、irqchip、FDT
4. `vstate.rs` 创建并配置 vCPU
5. `builder.rs` 将上述组件组装成可以启动的 microVM

### 1.2 与 LoongArch 适配直接相关的公共入口

当前代码里，LoongArch 适配的主入口主要有：

- `src/arch/src/loongarch64/mod.rs`
- `src/arch/src/loongarch64/layout.rs`
- `src/arch/src/loongarch64/linux/efi.rs`
- `src/arch/src/loongarch64/linux/regs.rs`
- `src/devices/src/fdt/loongarch64.rs`
- `src/devices/src/legacy/irqchip.rs`
- `src/devices/src/legacy/kvmloongarchirqchip.rs`
- `src/devices/src/legacy/loongarch64/serial.rs`
- `src/devices/src/virtio/mmio.rs`
- `src/vmm/src/device_manager/kvm/mmio.rs`
- `src/vmm/src/linux/vstate.rs`
- `src/vmm/src/builder.rs`

## 2. LoongArch 代码结构

### 2.1 `arch` 层

#### `src/arch/src/loongarch64/layout.rs`

定义 LoongArch 平台的静态布局常量，例如：

- RAM 起始地址 `DRAM_MEM_START = 0x4000_0000`
- MMIO 起始地址 `MAPPED_IO_START = 0x0a00_0000`
- 命令行、FDT、EFI system table 的保留大小
- 当前用于 CPU hardware interrupt 的 IRQ 范围 `IRQ_BASE = 2`、`IRQ_MAX = 9`

这里的 `2..9` 不是板级 PIC 号，而是当前 LoongArch 平台直接使用的 CPU HWI 范围。

#### `src/arch/src/loongarch64/mod.rs`

负责把布局常量落成运行时内存信息：

- 计算 RAM 尾地址
- 计算共享内存对齐地址
- 计算 FDT 地址
- 计算 EFI system table 地址
- 计算 initrd 地址

当前 LoongArch 的布局顺序是：

`RAM ... [EFI system table] [FDT]`

如果带 `initrd`，则 `initrd` 放在 EFI system table 之前。

#### `src/arch/src/loongarch64/linux/regs.rs`

负责 LoongArch vCPU 寄存器初始化：

- 通过 `setup_cpucfg()` 设置 guest 可见的 CPUCFG
- 设置 `pc`
- 设置 `a0/a1/a2`

当前约定是：

- `a0`：是否 EFI 启动
- `a1`：内核命令行地址
- `a2`：EFI system table 地址

也就是说，LoongArch 当前走的是“外部内核 + EFI handoff”风格的启动约定。

#### `src/arch/src/loongarch64/linux/efi.rs`

负责在 guest 内存中构造最小 EFI system table，并把 FDT 指针挂到 EFI config table 上。当前写入的核心对象有：

- EFI system table
- FDT 对应的 EFI config table
- vendor string

这使得 guest 内核可以通过 EFI handoff 找到 FDT。

### 2.2 `devices` 层

#### `src/devices/src/fdt/loongarch64.rs`

负责生成 LoongArch FDT。当前 FDT 有几个关键点：

- 根节点 `compatible = "linux,dummy-virt"`
- CPU interrupt controller 使用 `cpuintc`
- 仍保留 `eiointc` 和 `pch-pic` 节点作为平台兼容骨架
- 但当前 serial/virtio 设备已经直接挂到 `cpuintc`

这点非常关键：当前 LoongArch 平台已经不再让 serial/virtio 主要依赖 `pch-pic/eiointc` 递送中断，而是直接用 CPU interrupt 编号。

#### `src/devices/src/legacy/kvmloongarchirqchip.rs`

负责 LoongArch irqchip 设备对象。

它当前做了两件事：

1. 仍创建 KVM in-kernel 的 `IPI / EIOINTC / PCHPIC` 设备  
   这是为了保留 LoongArch 平台骨架
2. 对 serial/virtio 的实际中断注入，改为使用 `KVM_INTERRUPT`

这里最核心的变化是：

- 构造函数接收一个 `irq_vcpu_fd`
- `set_irq_state()` 通过这个 vCPU fd 调 `KVM_INTERRUPT`
- 正数表示 assert，负数表示 deassert

因此，当前 LoongArch 设备主路径已经不再依赖 `irqfd -> GSI -> 外部 irqchip` 这条链。

#### `src/devices/src/legacy/loongarch64/serial.rs`

这是 LoongArch 的 8250 风格串口实现。当前它已经完成了两件重要迁移：

1. 设备对象保存 `intc` 和 `irq_line`
2. 串口中断不再只是简单 `eventfd.write(1)`，而是通过 `sync_interrupt()` 根据当前 pending 状态调用 `set_irq_state()`

也就是说，serial 当前已经和 virtio 一样，采用“状态驱动的 assert/deassert”模型。

#### `src/devices/src/virtio/mmio.rs`

这是 LoongArch 适配里最关键的一个文件，因为 virtio used/config interrupt 的行为最终都落在这里。

当前 LoongArch 分支已经做了两件事：

1. `try_signal()` 只有在 `interrupt status` 从 `0 -> 非0` 时才 assert
2. guest ACK 后，如果 `interrupt status` 变回 `0`，才 deassert

此外，当前实现还加了一把只在 LoongArch 编译进去的同步锁，把：

- `fetch_or + assert`
- `fetch_and + deassert`

串成同一个临界区，避免 ACK 和新 pending 并发时把中断线错误拉低。

### 2.3 `vmm` 层

#### `src/vmm/src/device_manager/kvm/mmio.rs`

负责把 MMIO 设备注册到 KVM VM。

LoongArch 当前与其他架构的重要差异是：

- virtio：LoongArch 不再 `register_irqfd`
- serial：LoongArch 也不再 `register_irqfd`
- 对于 LoongArch，MMIO manager 只负责：
  - 注册 ioevent
  - 分配 `irq_line`
  - 把 `intc` 和 `irq_line` 写回设备对象

这意味着 LoongArch 设备的中断主路径已经完全转向 `set_irq_state() -> KVM_INTERRUPT`。

#### `src/vmm/src/linux/vstate.rs`

这里有 3 个 LoongArch 关键点：

1. `Vcpu::new_loongarch64()`  
   创建 LoongArch vCPU，并挂上 IOCSR 状态
2. `Vcpu::configure_loongarch64()`  
   调用 `arch::loongarch64::regs::setup_regs()`
3. `Vcpu::try_clone_irq_vcpu_file()`  
   克隆一个专门用于中断注入的 vCPU fd，交给 LoongArch irqchip

这第三点非常重要，因为当前 LoongArch 主中断路径不再走 VM fd 的 irqfd/routing，而是直接走 vCPU fd 的 `KVM_INTERRUPT`。

#### `src/vmm/src/builder.rs`

这是最终把 LoongArch 主线串起来的地方。当前 builder 的 LoongArch 分支主要做：

1. 计算 LoongArch 的 cmdline 地址
2. 创建 LoongArch vCPUs
3. 从 `vcpus[0]` clone 一个 irq 专用 fd
4. 用这个 fd 构造 `KvmLoongArchIrqChip`
5. 加载外部 LoongArch 内核

这里还有一个当前实现特有的点：

- LoongArch `KernelFormat::PeGz` 当前同时支持两种输入：
  - `PE + gzip` 封装的内核文件
  - 纯 `PE` 的 `vmlinux.efi`
- 无论输入是哪一种，最终都会校验 Linux LoongArch image header magic
- 然后用 `load_offset` 和固定 `VMLINUX_LOAD_ADDRESS` 推导 guest 中的 image load address 和 entry address

这点是当前代码和早期 bring-up 阶段的一个重要差异：

- 早期主要围绕 `vmlinuz.efi` / `PE+GZ` 路径调试
- 当前则已经补上了 plain `vmlinux.efi` 支持

这样做的直接原因是：当前 `/home/yzw/python-trans/linux` 这棵树默认稳定产物是 `arch/loongarch/boot/vmlinux.efi`，而不是 `vmlinuz.efi`。

## 3. 适配思路

LoongArch 适配不是“把文件补齐”就结束了，而是按照一条很明确的工程路线推进的。

### 3.1 第一步：先把最小可启动平台搭起来

第一阶段的目标不是设备都工作，而是 guest 至少能启动到内核早期阶段。

这一阶段的关键点是：

- 先定义内存布局
- 先把 LoongArch 外部内核加载起来
- 先把 vCPU 寄存器、EFI handoff、FDT 这三件事接上

对应实现分别是：

- `layout.rs` / `mod.rs`
- `builder.rs` 的 LoongArch `PeGz` 分支
- `regs.rs` / `efi.rs` / `fdt/loongarch64.rs`

### 3.2 第二步：先用 libkrun 现有公共模型接设备

第二阶段一开始，LoongArch 也是沿着 libkrun 其他架构的通用模型接设备：

- virtio-mmio 设备注册到 KVM MMIO manager
- 设备完成后通过 `eventfd`
- 再通过 `irqfd` 把中断递送给 guest

这条思路对 aarch64/riscv64/x86_64 是合理的，因为这些架构的 host KVM 外部中断路径足够完整。

### 3.3 第三步：识别 LoongArch 旧 host KVM 的特殊约束

实际 bring-up 过程中，LoongArch 暴露出一个关键差异：

- host 侧可以看到设备已经 signal 了中断
- 但 guest 侧迟迟进不了 `virtio-mmio` 的中断处理

这说明问题不在 FUSE reply 本身，而在“外部中断递送链”。

因此，当前代码的设计选择变成：

- 保留 LoongArch 的外部 irq 平台骨架
- 但对 serial/virtio 的主路径，不再依赖外部 irqchip 递送

### 3.4 第四步：把设备主路径切到 `cpuintc + KVM_INTERRUPT`

这是当前 LoongArch 适配最核心的设计变化。

具体做法是：

1. FDT 中把 serial/virtio 的 `interrupt-parent` 改成 `cpuintc`
2. IRQ 编号直接用 CPU HWI 范围
3. 设备模型内部改成显式的 `set_irq_state(active)`
4. LoongArch irqchip 最终用 vCPU fd 调 `KVM_INTERRUPT`

这样一来，主路径就变成：

`device model -> set_irq_state(active) -> KVM_INTERRUPT -> guest cpuintc`

而不是：

`device model -> eventfd/irqfd -> GSI/routing -> pch-pic/eiointc -> guest`

### 3.5 第五步：把“打一枪”模型改成“电平状态”模型

一旦切到 `KVM_INTERRUPT`，设备中断就不再适合“来一次事件打一次脉冲”的模型，而必须改成：

- 什么时候 assert
- 什么时候 deassert

当前代码中：

- virtio 通过 `interrupt status` 驱动 assert/deassert
- serial 通过 `interrupt_identification` 驱动 assert/deassert

这一步是当前 LoongArch 适配能稳定工作的关键。

### 3.6 第六步：把 bring-up 从“内核能起来”推进到“用户态能交互”

这一阶段不是再改 KVM 或中断主链，而是处理：

- rootfs 能不能真正作为 `/`
- `/init` 能不能跑起来
- 控制台到底走 `ttyS0` 还是 `hvc0`

当前开发态选择是：

- `ttyS0`
  - 保留给内核早期日志和 early console
- `hvc0`
  - 作为用户态交互 shell 的主要控制台

对应的经验结论是：

1. 内核 `console=` 不应只切到 `hvc0`
   否则在某些调试场景下，virtio-console 自身日志可能与控制台输出互相干扰
2. 用户态 `/init` 可以优先选择 `hvc0`
3. 如果希望 shell 拥有正常的 controlling tty，需要在新 session 中重新打开终端设备
   当前脚本采用了 `setsid` 方案

### 3.7 第七步：识别“guest 关机能力”与“shell 退出行为”是两件不同的问题

这一步很容易被忽略，但对后续维护很重要：

- shell 退出后不应直接让 PID 1 死掉，否则会触发 `Attempted to kill init!`
- 但当前 LoongArch 平台也还没有真正完善的 guest poweroff/reboot ABI

当前原因包括：

- LoongArch guest 的 `machine_power_off()` 需要板级 `pm_power_off` / sys-off handler，或 EFI runtime `ResetSystem()`
- 当前 libkrun LoongArch 的 EFI handoff 只提供了最小 system table，不提供 runtime services 指针
- 当前虚拟平台也没有专门的 poweroff 设备

因此，当前开发态 `/init` 的合理策略不是“强行关机”，而是：

- shell 退出后记录 `rc`
- 再拉起一个新的 shell

这属于当前平台能力边界，不是 `/init` 脚本实现质量问题。

## 4. 当前实现展开

### 4.1 内存与启动链

当前 LoongArch 的启动链是：

1. `arch_memory_regions()` 计算 RAM/FDT/EFI/initrd 布局
2. `builder.rs` 解析外部 `vmlinuz.efi`
3. LoongArch `PeGz` 分支解出 gzip 内核映像
4. `regs.rs` 设置 `pc/a0/a1/a2`
5. `efi.rs` 构造 EFI system table 和 FDT config table
6. guest 从 EFI handoff 进入内核

这条链的关键不是固件模拟，而是“最小 EFI handoff + 直接内核启动”。

### 4.2 FDT 与平台描述

当前 FDT 的设计原则是：

- 平台整体仍然保持 LoongArch 兼容骨架
- 主要设备描述为 libkrun 真实模拟的设备

因此现在的 FDT 同时满足两点：

- 仍保留 `eiointc/pch-pic`
- 但 serial 用 `ns16550a`
- virtio 用 `virtio,mmio`
- serial/virtio 中断直接挂到 `cpuintc`

这不是板级仿真，而是“LoongArch generic virt platform”的实现路线。

### 4.3 中断路径

当前 LoongArch 中断路径可分成两部分看：

#### 平台兼容骨架

- KVM in-kernel IPI
- KVM in-kernel EIOINTC
- KVM in-kernel PCHPIC
- FDT 中的 `eiointc` / `pch-pic`

这部分目前保留，是为了让平台描述仍然带有 LoongArch 外部中断骨架。

#### 设备实际主路径

serial/virtio 的实际中断路径已经是：

- serial：`sync_interrupt() -> set_irq_state() -> KVM_INTERRUPT`
- virtio：`try_signal()/ACK -> set_irq_state() -> KVM_INTERRUPT`

这也是为什么当前 LoongArch 的 MMIO manager 不再给 serial/virtio 注册 irqfd。

### 4.4 serial 适配

当前 LoongArch serial 的适配思路是：

1. 先保留原有 8250 模型
2. 再把设备对象补齐 `intc` 和 `irq_line`
3. 最后把中断触发改成显式状态同步

这比一开始直接删掉 `interrupt_evt` 更稳，因为它允许设备模型在迁移过程中保留 fallback。

### 4.5 virtio 适配

virtio 是当前 LoongArch bring-up 里最早暴露问题的部分，因此也是适配最深的一部分。

当前 virtio 路径的关键特征是：

- LoongArch 不再 `register_irqfd`
- `InterruptTransport` 维护 `interrupt status`
- `try_signal()` 只在 `0 -> 非0` 时 assert
- ACK 后 `非0 -> 0` 才 deassert
- LoongArch 分支额外串行化 `status` 与中断线更新，避免 deassert race

### 4.6 控制台与 `/init` 的当前工作方式

这一部分不是 strictly “架构 bring-up” 的底层代码，但它对 LoongArch 的可调试性非常关键。

当前开发态工作方式是：

- `examples/smoke_kernel.c`
  - kernel cmdline 使用 `console=ttyS0 console=hvc0`
  - serial console 仍保留输出到 host stdout
  - 但 serial 输入显式关闭（传 `-1`），避免继续消费 host stdin
- `examples/rootfs_debian_unstable/init`
  - 启动后先挂载 `/dev`、`/proc`、`/sys`
  - 通过 `/dev/kmsg` 打标记日志
  - 优先选择 `/dev/hvc0`，回退 `/dev/ttyS0`
  - 使用 `setsid` 拉起交互 shell，解决 controlling tty 问题
  - shell 退出后不会直接结束 PID 1，而是重新拉起 shell

这部分内容虽然不在 `src/` 主代码树里，但它是把 LoongArch bring-up 从“能启动”推进到“能交互调试”的关键工作。

## 5. 与其他架构的关系

需要特别说明的是：

- aarch64、riscv64、x86_64 当前大多仍沿用 `irqfd/eventfd`
- LoongArch 当前之所以不同，不是因为平台模型必须特殊
- 而是因为当前代码需要兼容旧 host KVM 环境下更保守的外部中断能力

因此：

- 平台建模上，LoongArch 仍然应该向 generic virt platform 靠拢
- 中断注入后端上，当前 LoongArch 采用了不同于其他架构的实现

## 6. 当前状态

截至当前代码，LoongArch 适配已经形成如下状态：

- 外部内核 `vmlinux.efi` / `PE+GZ` 内核均可加载
- LoongArch vCPU 初始化可完成
- EFI handoff 与 FDT 已接通
- serial/virtio 已切到 `cpuintc + KVM_INTERRUPT`
- LoongArch virtio 的 assert/deassert race 已做同步保护
- `eiointc/pch-pic` 仍保留为平台兼容骨架
- `hvc0` 用户态交互 shell 已可用
- `ttyS0` 仍保留给早期启动日志

但也要明确当前仍有两类“已知不完整”：

1. guest 侧真正的 poweroff/reboot ABI 尚未补齐
2. 当前 `/init` 与 shell 行为仍是开发态实现，而非产品化 init 流程

也就是说，当前 LoongArch 不是“只有最小能跑”，而是已经形成了一套清晰的、可继续收敛的实现框架。

## 7. 后续维护建议

如果后续继续演进 LoongArch 代码，建议遵循以下顺序：

1. 先保持启动链稳定  
   不要轻易动 `layout.rs`、`PeGz` 解析、EFI handoff
2. 再保持设备模型和 FDT 一致  
   设备挂到哪里，中断就应该从哪里注入
3. 再决定是否继续保留 `eiointc/pch-pic` 骨架  
   这是平台建模问题，不是当前功能主路径问题
4. 若 host KVM 后续补齐外部 irq 路径，再重新评估是否回归 irqfd  
   但那将是另外一轮设计选择，而不是当前代码的默认前提

## 8. 验证建议

当前 LoongArch 适配的验证顺序建议是：

1. 先编译 `devices` 和 `vmm`
2. 再运行 `examples/smoke_kernel`
3. 先确认 loader 走到了正确分支：
   - `PE+GZ`
   - 或 plain `vmlinux.efi`
4. 再看：
   - `loongarch efi handoff`
   - FDT 中 serial/virtio 的 IRQ 建模
5. 再看中断链是否真正打通：
   - guest 是否进入 `vm_interrupt`
   - guest 是否能完成 `virtio-fs` 初始化
6. 最后才看 guest 用户态，例如 `/init` 和 shell

按这个顺序排障，能最快把问题从“平台没起来”收敛到“设备/用户态自身问题”。

如果只是做回归测试，而不是再次 bring-up，建议直接参考：

- `docs/LOONGARCH_SMOKE_TEST.md`

这份文档记录了当前已验证通过的最小 smoke test。
