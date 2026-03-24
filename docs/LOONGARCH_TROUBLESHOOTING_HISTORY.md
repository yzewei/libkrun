# LoongArch 排障与修复记录

本文档记录本轮 LoongArch bring-up 中已经定位并解决的问题，目的是在后续继续开发或重新压缩上下文时，快速恢复关键背景。

它和 `docs/LOONGARCH_PORTING.md` 的分工不同：

- `LOONGARCH_PORTING.md`
  面向当前实现结构，讲“代码现在是怎样组织和工作的”
- `LOONGARCH_TROUBLESHOOTING_HISTORY.md`
  面向问题历史，讲“这次是怎么一步步从故障走到当前状态的”

## 1. 本轮最终结论

本轮最重要的结论有 4 个：

1. 旧 host KVM 下，LoongArch 的 `irqfd/eventfd -> 外部 irqchip` 主路径不可靠，不能作为 serial/virtio 的默认中断递送模型。
2. 对当前 libkrun，LoongArch 更稳的主路径是：`cpuintc + KVM_INTERRUPT`。
3. serial 和 virtio 都已经迁到“状态驱动的 assert/deassert”模型，不再只是打一枪的 pulse。
4. 中断问题解决后，virtio-fs 已经能够完成 `FUSE_INIT` 并挂载根文件系统；后续如果再出错，优先看 guest 用户态（例如 `/init`），而不是先怀疑中断。

## 2. 问题 1：virtio-fs 初始化 reply 已写出，但 guest 不进 `vm_interrupt`

### 2.1 现象

最初的典型日志是：

- host 侧看到 `FUSE_INIT ok`
- host 侧看到 `virtiofs reply bytes_written=80`
- host 侧看到 `signal_used_queue`
- 但 guest 侧始终没有：
  - `virtio-mmio: vm_interrupt`
  - `virtio-fs: vq_done`
  - `fuse: init reply`

这说明：

- FUSE reply 内容本身已经写到 guest buffer
- 问题不在 virtio-fs 协议体生成
- 问题在“中断没被 guest 消费到”

### 2.2 第一轮判断

当时先验证了几条链：

1. guest 是否真的跑到了新内核  
   通过在 kernel 里加 `virtio-fs: before fuse_send_init` 验证
2. guest virtio-mmio 队列是否已建立  
   通过 `find_vqs irq=... dev=26/3` 验证
3. guest IRQ controller 是否已 `set_type/unmask`  
   通过 `pch-pic: set_type/unmask` 验证
4. host 是否真的发了完成中断  
   通过 `signal_used_queue`、`eventfd write ok` 验证

结论是：

- FUSE backend 是通的
- guest virtqueue 建立是通的
- guest IRQ domain 配置也是通的
- 但 guest 仍然不进 `vm_interrupt`

因此问题被收敛到：

`host userspace -> KVM -> 外部 irqchip -> guest`

这条外部中断递送链，而不是 FUSE 内容本身。

## 3. 问题 2：`irqfd` 不行，改成 `KVM_IRQ_LINE` 直打 PCH-PIC 也不行

### 3.1 做过的实验

为了判断问题是不是只出在 `irqfd`，中间做过一个对照实验：

- 不再只依赖 `eventfd -> irqfd`
- 改成通过 `vmfd + KVM_IRQ_LINE` 直接给 PCH-PIC 打一根线

运行结果是：

- `KVM_IRQ_LINE assert ok`
- `KVM_IRQ_LINE deassert ok`
- 但 guest 还是没有 `virtio-mmio: vm_interrupt`

### 3.2 这个实验说明了什么

这个实验非常关键，因为它证明：

- 问题不是单纯 `eventfd` 没写成功
- 也不是单纯 `irqfd` 用户态入口没走到

更准确的说法是：

- `irqfd`
- `KVM_IRQ_LINE`

这两条路虽然入口不同，但它们都还依赖同一条“外部中断控制器递送链”：

`userspace -> KVM -> GSI/routing -> PCH-PIC/EIOINTC -> guest`

因此，当旧 host KVM 的 LoongArch 外部 irq 路径不完整时，这两条路都会失败。

### 3.3 为什么推断旧 host KVM 的外部 irq 路径不完整

这个判断不是凭感觉，而是结合源码做出的：

- 旧内核树里有 generic `eventfd.c` / `irqchip.c`
- 但缺少 LoongArch 专门的：
  - `kvm_set_routing_entry`
  - `kvm_arch_set_irq_inatomic`
  - `kvm_vm_ioctl_irq_line`
- 新内核树 `linux-loongson` 里补上了这些 LoongArch 相关实现

所以最合理的工程结论是：

旧 host KVM 不是“virtio-fs reply 格式不对”，而是“LoongArch 外部 irq 路径支撑不完整”。

## 4. 解决思路：从 `pch-pic/eiointc` 主路径切到 `cpuintc + KVM_INTERRUPT`

### 4.1 为什么选 `KVM_INTERRUPT`

旧 host KVM 虽然外部 irq 路径不可靠，但 LoongArch 的 vCPU 直注入中断是可用的。

LoongArch KVM 对 `KVM_INTERRUPT` 的语义是：

- 正数：assert
- 负数：deassert

而 guest 的 CPU interrupt controller 是 onecell 模型，所以可以直接把设备 IRQ 建模成 CPU hardware interrupt。

因此最终方案不是继续死磕外部 irqchip，而是：

- serial/virtio 在 FDT 里直接挂到 `cpuintc`
- 中断编号直接使用 CPU HWI 范围
- VMM 通过 vCPU fd 调 `KVM_INTERRUPT`

### 4.2 当前采用的 IRQ 号范围

当前 LoongArch 的设备 IRQ 号已经改成 CPU HWI 风格：

- `IRQ_BASE = 2`
- `IRQ_MAX = 9`

这不是板级 PIC 号，而是 guest CPU interrupt 编号范围。

## 5. 问题 3：virtio 原先是 pulse 语义，不适合 `KVM_INTERRUPT`

### 5.1 原始问题

原来的 virtio-mmio 中断语义更接近“打一枪”：

- 设备完成后置 `interrupt status`
- 然后立刻 `set_irq(...)`

但切到 `KVM_INTERRUPT` 之后，这样不够，因为：

- assert 和 deassert 必须明确区分
- 只打脉冲不表达“当前线是否仍然 active”

### 5.2 当前修正

`src/devices/src/virtio/mmio.rs` 已改成：

- `interrupt status` 从 `0 -> 非0` 时才 assert
- guest ACK 后，如果 `status` 从 `非0 -> 0` 才 deassert

也就是：

- `try_signal()` 负责 assert
- MMIO ACK (`0x64`) 负责 deassert

### 5.3 后续又修掉了 deassert race

初版 deassert 逻辑仍有 race：

1. ACK 线程 `fetch_and(!ack)`
2. 计算 `old/new`
3. 若 `new == 0`，执行 deassert

但在 `fetch_and()` 和 `deassert` 之间，如果另一个线程又 `fetch_or()` 置位了新的 pending bit，中断线可能被错误拉低。

当前修正方式是：

- 在 `InterruptTransport` 中增加一把 LoongArch 专用同步锁
- 把：
  - `fetch_or + assert`
  - `fetch_and + deassert`
  串成同一个临界区

这样主路径上的 LoongArch virtio deassert race 已被收掉。

## 6. 问题 4：LoongArch serial 仍然停留在旧 `eventfd` 路径

### 6.1 原始状态

最开始 serial 的状态和 FDT 是不一致的：

- FDT 已经把 serial 挂到 `cpuintc`
- 但 LoongArch serial 设备模型里：
  - 不保存 `intc`
  - 不保存 `irq_line`
  - 触发中断仍然只是 `interrupt_evt.write(1)`

### 6.2 当前修正

`src/devices/src/legacy/loongarch64/serial.rs` 已做如下迁移：

1. 串口对象保存 `intc` 和 `irq_line`
2. `register_mmio_serial()` 在注册设备时把这两个状态写回 serial
3. serial 通过 `sync_interrupt()` 根据当前 pending 状态调用 `set_irq_state(...)`

也就是说，serial 现在和 virtio 一样，已经切到：

- 状态驱动
- assert/deassert 分离
- 最终走 `KVM_INTERRUPT`

## 7. 问题 5：LoongArch KVM MMIO manager 里还残留 irqfd 注册

### 7.1 原始状态

即使 LoongArch 已经切到 `KVM_INTERRUPT` 主路径，KVM MMIO manager 仍然会像其他架构那样：

- 给 virtio `register_irqfd`
- 给 serial `register_irqfd`

这会让代码语义看起来仍像“依赖 irqfd”。

### 7.2 当前修正

现在 LoongArch 分支已经显式跳过：

- virtio 的 `register_irqfd`
- serial 的 `register_irqfd`

因此当前 LoongArch 的 MMIO manager 职责已经变成：

- 注册 ioevent
- 分配 `irq_line`
- 把 `intc/irq_line` 写回设备对象

而不是依赖 `irqfd` 做主中断递送。

## 8. 问题 6：FDT 与中断模型需要同步更新

### 8.1 走过的弯路

中间做过一个 `EDGE_RISING` 的实验，希望用 edge 触发掩盖旧 KVM 外部 irq 路径的问题。

这个实验最终被放弃，原因是：

- 根因不在 edge/level 的编码本身
- 走 `pch-pic/eiointc` 主路径本身就不稳

### 8.2 当前状态

当前 FDT 的处理方式是：

- 保留 `eiointc` / `pch-pic` 节点作为兼容骨架
- 但 serial/virtio 已直接挂到 `cpuintc`
- 设备 IRQ 用 onecell `<N>` 描述，而不是 `<irq type>`

这意味着：

- 对 serial/virtio，当前已经不再讨论 `irq_type=edge/level`
- 因为它们走的是 `cpuintc`，不是 `pch-pic`

## 9. 问题 7：LoongArch 中断修好以后，真正暴露出的下一个问题不是中断

中断链打通以后，日志已经能看到：

- guest 进入 `virtio-mmio: vm_interrupt`
- virtio-fs 完成 `FUSE_INIT`
- guest 成功挂载 `virtiofs` 根文件系统

因此后续如果 guest 仍然 panic，例如：

- `Run /init as init process`
- `Attempted to kill init!`

那么优先应检查：

- rootfs 里的 `/init`
- rootfs 依赖的解释器和用户态环境

而不是再回头先怀疑 LoongArch 中断路径。

## 10. 编译链路里顺手修掉的问题

本轮除运行时问题外，还顺手修掉了几类编译/兼容性问题：

### 10.1 vCPU fd clone helper 的类型链

为了把 `KVM_INTERRUPT` 接到 LoongArch irqchip，需要从 `Vcpu` clone 一个 irq 专用 fd。

这带来了几处实现细节修正：

- `try_clone_irq_vcpu_file()` 使用 `libc::dup(self.fd.as_raw_fd())`
- 使用 `File::from_raw_fd(...)` 封装 cloned fd
- 在 `vstate::Error` 里增加 `VcpuFdClone(io::Error)`
- `builder.rs` 通过 `Error::Vcpu(...)` 把它接回 `StartMicrovmError`

### 10.2 `cpuid` crate 与新 Rust 的 `unsafe_op_in_unsafe_fn`

新 Rust 版本下，`__get_cpuid_max`、`__cpuid_count`、`__cpuid` 需要显式包进 `unsafe`。

这部分已在：

- `src/cpuid/src/common.rs`
- `src/cpuid/src/brand_string.rs`

中补齐。

## 11. 当前代码状态

截至本轮结束，LoongArch 当前代码状态可以概括为：

- 启动链已接通：`vmlinuz.efi` -> PE/GZ 解析 -> EFI handoff -> guest kernel
- FDT 已形成当前平台描述：`cpuintc` 为主，`eiointc/pch-pic` 为保留骨架
- virtio 主中断路径已迁到 `KVM_INTERRUPT`
- serial 主中断路径已迁到 `KVM_INTERRUPT`
- LoongArch MMIO manager 已不再给 serial/virtio 注册 irqfd
- virtio deassert 的主 race 已修

## 12. 仍可继续关注的遗留点

以下项目不是本轮主阻塞，但后续可以继续清理：

1. `eiointc/pch-pic` 现在仍保留在平台骨架中  
   是否长期保留，取决于后续平台建模选择
2. serial 里 `interrupt_evt` 现在已不是主路径，只是 fallback/兼容残留  
   后续可以再决定是否进一步瘦身
3. `virtio/mmio.rs` 的 `BusDevice::interrupt()` 入口仍保持旧风格  
   如果未来 LoongArch 会走到这条路径，需要继续和主路径同步
4. 用户态 `/init` 失败问题属于下一阶段  
   不属于当前中断 bring-up 范围

## 13. 推荐的后续阅读顺序

如果下次要快速恢复上下文，推荐按这个顺序读：

1. 本文档  
   先恢复“问题历史”和“为什么这样改”
2. `docs/LOONGARCH_PORTING.md`  
   再恢复“当前代码结构和实现关系”
3. 关键实现文件  
   - `src/devices/src/legacy/kvmloongarchirqchip.rs`
   - `src/devices/src/virtio/mmio.rs`
   - `src/devices/src/legacy/loongarch64/serial.rs`
   - `src/devices/src/fdt/loongarch64.rs`
   - `src/vmm/src/device_manager/kvm/mmio.rs`
   - `src/vmm/src/linux/vstate.rs`
   - `src/vmm/src/builder.rs`
