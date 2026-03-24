# LoongArch Smoke Test

本文档记录当前 LoongArch 在 libkrun 上的最小可用性回归测试。

目标不是覆盖所有功能，而是快速回答两个问题：

1. 当前修改是否把 guest 启动链打坏了？
2. 当前修改是否把基础设备和根文件系统打坏了？

当前这份 smoke test 基于如下事实：

- host KVM 仍是旧版 LoongArch KVM
- libkrun 的 LoongArch 中断主路径已经切到 `cpuintc + KVM_INTERRUPT`
- guest kernel 使用 `/home/yzw/python-trans/linux/arch/loongarch/boot/vmlinux.efi`
- rootfs 使用 `examples/rootfs_debian_unstable`
- kernel cmdline 当前采用：
  - `console=ttyS0 console=hvc0 root=/dev/root rootfstype=virtiofs rw init=/init`

## 1. 测试前提

需要先准备：

- libkrun 已编译完成
- `examples/smoke_kernel` 已重新编译
- guest kernel 已重新编译为 `vmlinux.efi`
- `examples/rootfs_debian_unstable/init` 已能把用户态 shell 切到 `hvc0`

推荐运行命令：

```bash
cd /home/yzw/python-trans/libkrun/examples
RUST_BACKTRACE=full LD_LIBRARY_PATH=../target/release \
./smoke_kernel \
/home/yzw/python-trans/linux/arch/loongarch/boot/vmlinux.efi \
./rootfs_debian_unstable
```

## 2. 启动判定

看到下面这些关键信号，就说明启动主链已经通了：

- `VFS: Mounted root (virtiofs filesystem) on device 0:19.`
- `Run /init as init process`
- `/init` 脚本中写入 `/dev/kmsg` 的标记，例如：
  - `init: guest /init reached`
- 用户态控制台出现：
  - `=== starting shell ===`
  - `#`

如果已经看到 `#` 提示符，说明 guest 已经进入可交互 shell。

## 3. 第一组测试：基础可用性

在 guest shell 中执行：

```sh
echo '=== basic ==='
uname -a
cat /proc/cmdline
mount | grep ' / '
pwd
ls /

echo '=== console ==='
ls -l /dev/hvc0 /dev/ttyS0 /dev/console

echo '=== virtio ==='
ls /sys/bus/virtio/devices
cat /proc/interrupts

echo '=== fs ==='
touch /tmp/krun_test
echo hello > /tmp/krun_test
cat /tmp/krun_test
ls -l /init /bin/sh /etc
```

预期结果：

- `mount | grep ' / '` 能看到根文件系统是 `virtiofs`
- `/dev/hvc0`、`/dev/ttyS0`、`/dev/console` 都存在
- `/sys/bus/virtio/devices` 中能看到 virtio 设备
- `/tmp/krun_test` 可以创建、写入、读取
- `/init`、`/bin/sh`、`/etc` 可以正常访问

## 4. 第二组测试：稳定性与资源视图

在 guest shell 中继续执行：

```sh
echo '=== proc ==='
cat /proc/filesystems
cat /proc/mounts
cat /proc/interrupts

echo '=== memory cpu ==='
free -h
cat /proc/meminfo | head
nproc
cat /proc/cpuinfo | head -40

echo '=== write loop ==='
mkdir -p /tmp/krun_dir
for i in 1 2 3 4 5; do
  echo "line-$i" >> /tmp/krun_dir/test.txt
done
cat /tmp/krun_dir/test.txt

echo '=== shell io ==='
echo abc
ls /tmp
pwd
```

预期结果：

- `proc`、`sysfs`、`devtmpfs`、`virtiofs` 都在 mounts/filesystems 中出现
- `free -h`、`nproc`、`cpuinfo` 与当前 VM 配置基本一致
- 连续写文件后内容正确，没有卡死或 I/O 错误
- shell 交互正常，不会失去响应

## 5. 当前已知现象

以下现象目前不视为 smoke test 失败：

1. `ttyS0` 主要保留给早期内核日志

当前交互 shell 主要走 `hvc0`。`smoke_kernel` 已经把 serial 输入关闭，不再消费 host `stdin`。

2. shell 输入 `exit` 后会被 `/init` 重新拉起

当前开发态 `init` 脚本不会直接关机退出 VM，而是记录 shell 退出码后重新启动交互 shell。

3. `eiointc/pch-pic` 仍然保留在 FDT 和 irqchip 初始化里

它们当前是平台兼容骨架，serial/virtio 的主中断路径已经不是这条链。

## 6. 失败时优先排查

如果 smoke test 失败，建议按这个顺序判断：

1. 如果根本进不了 shell：
   先看 `VFS: Mounted root`、`Run /init as init process` 是否出现
2. 如果 `/init` 没输出：
   先看 `/dev/kmsg` 标记，再看解释器链 `/bin/sh -> dash -> ld-linux`
3. 如果 virtio-fs 初始化失败：
   先看 `FUSE_INIT` 和根目录路径是否正确
4. 如果 guest 不进 `vm_interrupt`：
   先看当前是否还在误走旧的 `irqfd/PCH-PIC` 路径

## 7. 当前判定标准

当本文档中的两组测试都通过时，可以把当前状态定性为：

`LoongArch on old host KVM 已完成基本 bring-up，启动链、根文件系统、基础设备与交互 shell 均可用。`
