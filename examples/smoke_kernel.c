#include <errno.h>
#include <libkrun.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

static void die_krun(const char *what, int ret)
{
    errno = -ret;
    perror(what);
    exit(1);
}
int main(int argc, char *argv[])
{
    if (argc < 2 || argc > 4) {
        fprintf(stderr, "Usage: %s KERNEL [ROOTFS_DIR] [CMDLINE]\n", argv[0]);
        return 1;
    }

    const char *kernel = argv[1];
    const char *rootfs_dir = (argc >= 3) ? argv[2] : NULL;
    //const char *initrd = (argc >= 3) ? argv[2] : NULL;
    const char *cmdline = (argc >= 4) ? argv[3] : "console=ttyS0 console=hvc0 root=/dev/root rootfstype=virtiofs rw init=/init";

    int ret;
    int32_t ctx_id;

    //ret = krun_set_log_level(KRUN_LOG_LEVEL_TRACE);
    ret = krun_set_log_level(KRUN_LOG_LEVEL_INFO);
    if (ret < 0) {
        die_krun("krun_set_log_level", ret);
    }

    ctx_id = krun_create_ctx();
    if (ctx_id < 0) {
        die_krun("krun_create_ctx", ctx_id);
    }

    ret = krun_set_vm_config(ctx_id, 1, 2048);
    if (ret < 0) {
        die_krun("krun_set_vm_config", ret);
    }

    // --- ✨ 在这里添加 VirtioFS 支持 ---
    if (rootfs_dir != NULL) {
        printf("Setting RootFS directory to: %s\n", rootfs_dir);
        // 这个函数会告诉 libkrun 把该目录通过 VirtioFS 共享给虚拟机
        ret = krun_set_root(ctx_id, rootfs_dir);
        if (ret < 0) die_krun("krun_set_root  ", ret);
    }
    // ---------------------------------

    // Keep early ttyS0 output, but do not consume host stdin. Passing -1
    // tells libkrun there is no serial input fd, which avoids epolling
    // /dev/null and keeps hvc0 as the interactive console.
    ret = krun_add_serial_console_default(ctx_id, -1, STDOUT_FILENO);
    if (ret < 0) {
        die_krun("krun_add_serial_console_default", ret);
    }

    ret = krun_set_kernel(
        ctx_id,
        kernel,
        KRUN_KERNEL_FORMAT_PE_GZ,
        NULL,
        cmdline
    );
    if (ret < 0) {
        die_krun("krun_set_kernel", ret);
    }

    ret = krun_start_enter(ctx_id);
    if (ret < 0) {
        die_krun("krun_start_enter", ret);
    }

    return 0;
}
