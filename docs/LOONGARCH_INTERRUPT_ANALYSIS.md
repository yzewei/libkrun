# LoongArch KVM 中断问题分析与解决方案

## 先说结论

这次碰到的问题，核心不是"virtio-fs 不会发中断"，而是：

**旧 LoongArch KVM 对"外部中断控制器那条虚拟板级链路"的支持不完整，导致 `irqfd` 和 `KVM_IRQ_LINE` 都进了 KVM，但没有把中断有效送到 guest 的 `virtio-mmio` handler。**

所以最后改成 `KVM_INTERRUPT + cpuintc`，本质上不是"瞎绕过去"，而是：

**把虚拟平台从"板级外部 PIC 模型"改成"更通用的 CPU interrupt 直连模型"。**

---

## 1. 原来的中断模型是什么

原来 libkrun 的 virtio-mmio 逻辑是：

1. 设备完成一个 virtqueue 操作后，在 `mmio.rs` 里置 `interrupt status`
2. 然后调用 irqchip 的 `set_irq(...)`
3. LoongArch 旧实现的 `set_irq(...)` 只是写 `eventfd`
4. `eventfd` 通过 `vm.register_irqfd(...)` 绑定到某个 GSI
5. KVM 再把这个 GSI 路由到 guest 里的 `PCH-PIC/EIOINTC`
6. 最后 guest 的 `virtio-mmio` 中断处理函数 `vm_interrupt()` 被调用

也就是这条链：

```
device model -> eventfd -> irqfd -> GSI/routing -> PCH-PIC/EIOINTC -> guest IRQ domain -> virtio-mmio handler
```

---

## 2. 为什么 `irqfd` 不行，而且"自己拿 vmfd 发 PCH PIC 中断"还是不行

这是最关键的逻辑点。

到底是：
- `GSI/irqchip -> PCH-PIC/EIOINTC`
- 还是 `PCH-PIC/EIOINTC -> guest`

哪段坏了？

**严格说：只靠运行日志，我们不能 100% 把断点钉到两者中的某一步。**  
但我们能把范围收得很小，而且源码证据更偏向前者，也就是：

**坏在 KVM 里"把 userspace 的 GSI/irq 输入变成 LoongArch 外部 irqchip 状态"这层 arch glue。**

原因是三条证据合起来看：

### 证据 1：`irqfd` 路径的弱实现

generic KVM 会先尝试 `kvm_arch_set_irq_inatomic()`；旧树的默认弱实现直接返回 `-EWOULDBLOCK`。  
这意味着 LoongArch 如果没有自己的实现，就只能退回慢路径。

```c
// virt/kvm/eventfd.c
kvm_set_irq_inatomic()
  -> kvm_arch_set_irq_inatomic()  // 旧 LoongArch 无实现，返回 -EWOULDBLOCK
```

### 证据 2：irqfd routing 依赖 arch hook

irqfd 的 routing 还依赖 `kvm_set_routing_entry()`；generic 框架会调用这个 arch hook。  
但旧树里查不到 LoongArch 自己的实现，只有新树才有。

```c
// virt/kvm/irqchip.c
kvm_set_routing_entry()
  -> kvm_arch_set_routing_entry()  // 旧 LoongArch 无实现
```

### 证据 3：`KVM_IRQ_LINE` 同样缺少 arch 实现

`KVM_IRQ_LINE` 这条 userspace 直打 VM fd 的路，旧树里同样查不到 LoongArch 自己的 `kvm_vm_ioctl_irq_line()` 实现。

```c
// arch/loongarch/kvm/vm.c (新树才有)
kvm_vm_ioctl_irq_line()  // 旧 LoongArch 无实现
```

### 综合判断

这三点放一起，最合理的判断是：

- userspace 把"我要打一根外部线"这件事交给 KVM 了
- generic KVM 框架也确实进去了
- 但 LoongArch 旧 KVM 缺少把这件事真正落实到 **外部虚拟 irqchip** 的 arch-specific 支撑

所以：

- `irqfd` 不行，不是 `eventfd` 自己不行
- `KVM_IRQ_LINE` 不行，也不是 `ioctl` 自己不行
- 它们失败的根因很可能是**共用的下游外部 irqchip 路径没有真正工作**

这就是为什么后来看到：

- `eventfd write ok`
- `KVM_IRQ_LINE assert ok/deassert ok`

但 guest 仍然没有 `vm_interrupt`

因为"调用成功"只说明**userspace -> KVM 这一步到了**，不说明**KVM -> guest 外部中断状态**真的生效了。

---

## 3. 为什么最后选 `KVM_INTERRUPT + cpuintc`

因为我们要绕开的，不是 `eventfd` 这一点，而是整条：

```
GSI / routing / PCH-PIC / EIOINTC
```

而旧内核里，LoongArch 对 **vCPU 直注入 CPU interrupt** 是本来就支持的：

### KVM_INTERRUPT ioctl

```c
// arch/loongarch/kvm/vcpu.c
KVM_INTERRUPT 收到 irq 后，会转成 int
  - 正数：kvm_queue_irq(vcpu, intr)
  - 负数：kvm_dequeue_irq(vcpu, -intr)
```

### LoongArch CPU hardware interrupt 号

```c
// arch/loongarch/include/asm/loongarch.h
INT_HWI0..INT_HWI7 = 2..9
```

### guest 的 CPU interrupt controller 是 onecell 映射

```c
// drivers/irqchip/irq-loongarch-cpu.c
cpuintc 使用 onecell 映射
```

所以新模型变成：

```
device model -> set_irq_state(active) -> KVM_INTERRUPT(vcpu) -> guest CPU pending HWIx -> Linux irq domain -> virtio-mmio handler
```

这条路不需要：
- irqfd
- GSI routing
- PCH-PIC
- EIOINTC

---

## 4. 名词解释：什么是 onecell

在设备树里，`interrupt-controller` 节点会定义 `#interrupt-cells`。

### onecell

表示一个中断只用 **1 个 32-bit cell** 描述。

例如 `cpuintc`：
```dts
interrupts = <6>;
```
这里的 `6` 就是 CPU hardware interrupt 号。

### twocell

一般是：
```dts
interrupts = <irq_number irq_type>;
```

例如 `pch-pic` 这种板级 PIC 常见的是：
```dts
interrupts = <4 IRQ_TYPE_LEVEL_HIGH>;
```

现在把 virtio 节点改到 `CPU_INTC_PHANDLE` 下以后，`interrupts` 就只剩一个数字了。  
所以这时不再谈 `edge/level` 那个第二个 cell。

---

## 5. 代码上我们到底改了什么

这次关键改动是 5 组。

### 改动 1：把 LoongArch IRQ 编号空间改到 CPU HWI 范围

```rust
// src/arch/src/loongarch64/layout.rs
pub const IRQ_BASE: u32 = 2;  // INT_HWI0
pub const IRQ_MAX: u32 = 9;   // INT_HWI7
```

### 改动 2：把 virtio 和 serial 的 FDT `interrupt-parent` 改到 `cpuintc`，并使用 onecell

```rust
// src/devices/src/fdt/loongarch64.rs
// virtio 节点
fdt.property_u32("interrupt-parent", CPU_INTC_PHANDLE)?;
fdt.property_array_u32("interrupts", &[irq])?;  // onecell，只有 irq 号

// serial 节点
fdt.property_u32("interrupt-parent", CPU_INTC_PHANDLE)?;
fdt.property_array_u32("interrupts", &[irq])?;
```

### 改动 3：给 irqchip 抽象加 LoongArch 专用的 `set_irq_state(..., active)`

```rust
// src/devices/src/legacy/irqchip.rs
pub trait IrqChipT {
    // 新增电平语义接口
    fn set_irq_state(&self, irq_line: u32, active: bool) -> Result<(), DeviceError>;
}
```

### 改动 4：把 virtio-mmio 从"打一枪"改成"电平语义"

```rust
// src/devices/src/virtio/mmio.rs
// status 从 0 变非 0 时 assert
if old_status == 0 && new_status != 0 {
    self.interrupt_evt.write(1)?;
}

// guest ACK 后，如果 status 清成 0，就 deassert
if status == 0 {
    self.interrupt_evt.write(0)?;
}
```

### 改动 5：给 vCPU clone 一个专门做中断注入的 fd，再在 LoongArch irqchip 里用 `KVM_INTERRUPT`

```rust
// src/vmm/src/linux/vstate.rs
// clone helper
pub fn clone_for_interrupt_injection(&self) -> Result<VcpuFd> {
    self.fd.clone_for_interrupt_injection()
}

// src/vmm/src/builder.rs
// builder 传入
vcpu_interrupt_fds.push(vcpu.fd.clone_for_interrupt_injection()?);

// src/devices/src/legacy/kvmloongarchirqchip.rs
// 最终注入
fn set_irq_state(&self, irq_line: u32, active: bool) -> Result<(), DeviceError> {
    let signed_irq = if active { irq_line as i32 } else { -(irq_line as i32) };
    self.vcpu_interrupt_fds[cpu_index].kvm_interrupt(signed_irq)?;
    Ok(())
}
```

---

## 6. 这个最终方案"合不合规"

要分目标看。

### 如果目标是"模拟某块 Loongson 板子"

那它**不板级保真**。  
因为真实板子通常是设备挂在外部 PIC / SoC interrupt fabric 上，不是直接挂 CPU intc。

### 如果目标是"做一个通用的 LoongArch virt 平台"

那这是**合理的，甚至更干净**。  
因为你并不是在做 2K2000 板级仿真，而是在做一个 VMM 定义的虚拟平台 ABI。

从这个角度看：
- 用 `ns16550a`
- 用 `virtio,mmio`
- 用 `cpuintc` 直接接虚拟设备中断

这是"虚拟平台设计"，不是"板卡复刻"。

### 有没有更好的思路？

有两个方向：

#### 方案 1：最板级保真的方案

补齐旧 host KVM 的 LoongArch 外部 irqchip/irqfd/routing 支持  
这样你就可以继续走 `PCH-PIC/EIOINTC` 模型  
这是最"像板子"的方案，但现实约束是不升级/不改 host KVM，所以**不可用**

#### 方案 2：最务实的通用 virt 方案

继续把 virtio 设备都迁到 `cpuintc + KVM_INTERRUPT`  
这是当前约束下**最可控的方案**

### 评价

- 对"板级仿真"来说，不是最合规
- 对"generic microVM / virt platform"来说，是合规且合理的

---

## 7. 正常设备中断链 vs 你现在这条链

"正常设备发出 int 信号，接收的是谁？"

### 物理机上通常是

```
设备 -> 中断控制器 -> CPU
```

### VMM 里其实是

```
设备模型 (在用户态 VMM 中) -> KVM -> guest 可见的中断拓扑 -> guest CPU
```

区别只在于**KVM 给 guest 暴露的中断拓扑是什么**。

### 原来的模型

```
设备模型 -> eventfd/irqfd -> KVM GSI -> guest PCH-PIC/EIOINTC -> guest CPU
```

### 现在的模型

```
设备模型 -> KVM_INTERRUPT(vcpu fd) -> guest CPU intc
```

所以并不是"设备直接发给 vCPU，跳过了 VMM"。  
而是：

- 设备模型仍然在 VMM 里决定"现在有 pending interrupt"
- VMM 再调用 KVM 注入
- 只不过以前注入的是"外部 irqchip 线"
- 现在注入的是"CPU hardware interrupt pending bit"

这就是本质区别。

---

## 8. 验证：这条链已经跑通

在最新日志里，已经看到了：

### host 侧

```
KVM_INTERRUPT ok: irq=6, signed_irq=6, active=true
KVM_INTERRUPT ok: irq=6, signed_irq=-6, active=false
```

### guest 侧

```
virtio-mmio: vm_interrupt irq=19
read interrupt status: 0x1
write interrupt ack: 0x1
```

### FUSE 挂载成功

```
FUSE_INIT ok
fuse: init complete ok=1
VFS: Mounted root (virtiofs filesystem)
```

这说明中断设计已经跑通了。

---

## 9. 中断模型对比

### 原模型：板级外部 PIC 模型

```
┌─────────────────────────────────────────────────────────────────┐
│  Userspace (libkrun)                                            │
│  device model -> eventfd -> set_irq()                           │
└────────────────────────┬────────────────────────────────────────┘
                         │ eventfd_write()
                         ▼
┌─────────────────────────────────────────────────────────────────┐
│  KVM (kernel)                                                   │
│  irqfd -> GSI routing -> kvm_set_irq_inatomic()                 │
│  ❌ 旧 LoongArch 缺少 arch 实现，返回 -EWOULDBLOCK               │
└────────────────────────┬────────────────────────────────────────┘
                         │ (路径失败)
                         ▼
┌─────────────────────────────────────────────────────────────────┐
│  Guest                                                          │
│  PCH-PIC -> EIOINTC -> CPU INTC -> CPU                          │
│  ❌ 中断从未到达这里                                            │
└─────────────────────────────────────────────────────────────────┘
```

### 新模型：CPU interrupt 直连模型

```
┌─────────────────────────────────────────────────────────────────┐
│  Userspace (libkrun)                                            │
│  device model -> set_irq_state(active) -> KVM_INTERRUPT ioctl   │
└────────────────────────┬────────────────────────────────────────┘
                         │ kvm_vcpu_ioctl(KVM_INTERRUPT, signed_irq)
                         ▼
┌─────────────────────────────────────────────────────────────────┐
│  KVM (kernel)                                                   │
│  kvm_queue_irq(vcpu, intr) / kvm_dequeue_irq(vcpu, -intr)       │
│  ✅ 旧 LoongArch 已支持 vCPU 直注入                              │
└────────────────────────┬────────────────────────────────────────┘
                         │ guest CPU pending HWIx
                         ▼
┌─────────────────────────────────────────────────────────────────┐
│  Guest                                                          │
│  CPU INTC (onecell) -> Linux irq domain -> virtio-mmio handler  │
│  ✅ 中断成功到达                                                │
└─────────────────────────────────────────────────────────────────┘
```

---

## 10. 遗留问题与后续优化

当前还有一些过渡态，不够干净：

### 10.1 serial 还是半迁移状态

- serial 现在仍然使用 `PCH_PIC_PHANDLE`
- 应该统一迁移到 `CPU_INTC_PHANDLE`

### 10.2 `cpuintc` 方案下哪些节点还能进一步收敛

- `KvmLoongArchIrqChip` 现在只用于 serial，可以考虑移除
- `irqchip.rs` 的抽象可以简化，去掉 GSI/routing 相关代码

### 10.3 `virtio-mmio` 现在 ACK/deassert 的 race 以后该怎么修

- 当前 ACK 和 deassert 之间可能有 race
- 需要考虑更严谨的状态机

---

## 参考资料

### 内核源码

- `arch/loongarch/kvm/vcpu.c` - KVM vCPU 中断处理
- `arch/loongarch/kvm/vm.c` - KVM VM ioctl 实现（新树）
- `arch/loongarch/kvm/irqfd.c` - irqfd 支持（新树）
- `drivers/irqchip/irq-loongarch-cpu.c` - CPU interrupt controller 驱动
- `virt/kvm/eventfd.c` - irqfd 通用框架
- `virt/kvm/irqchip.c` - irqchip 路由框架

### libkrun 源码

- `src/arch/src/loongarch64/layout.rs` - IRQ 编号定义
- `src/devices/src/fdt/loongarch64.rs` - FDT 中断节点
- `src/devices/src/legacy/irqchip.rs` - irqchip 抽象
- `src/devices/src/legacy/kvmloongarchirqchip.rs` - KVM LoongArch irqchip 实现
- `src/devices/src/virtio/mmio.rs` - virtio-mmio 中断逻辑
- `src/vmm/src/linux/vstate.rs` - vCPU 中断注入 fd clone
- `src/vmm/src/builder.rs` - VM 构建流程

---

*最后更新：2026 年 3 月*
