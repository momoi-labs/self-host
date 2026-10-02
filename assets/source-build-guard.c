/* Trusted Platform code. Repository code must never replace this launcher. */
#define _GNU_SOURCE
#include <linux/capability.h>
#include <stdio.h>
#include <sys/prctl.h>
#include <sys/syscall.h>
#include <unistd.h>

int main(int argc, char **argv) {
    uid_t real_uid, effective_uid, saved_uid;
    if (argc < 2 || getresuid(&real_uid, &effective_uid, &saved_uid) != 0 ||
        real_uid == 0 || effective_uid == 0 || saved_uid == 0) {
        fputs("source build guard requires nonzero real, effective and saved UIDs\n", stderr);
        return 126;
    }
    struct __user_cap_header_struct header = { _LINUX_CAPABILITY_VERSION_3, 0 };
    struct __user_cap_data_struct capabilities[2] = { {0}, {0} };
    if (syscall(SYS_capset, &header, capabilities) != 0 ||
        prctl(PR_CAP_AMBIENT, (unsigned long)PR_CAP_AMBIENT_CLEAR_ALL, 0UL, 0UL, 0UL) != 0) {
        fputs("source build guard could not drop capabilities\n", stderr);
        return 126;
    }
    if (prctl(PR_SET_NO_NEW_PRIVS, 1UL, 0UL, 0UL, 0UL) != 0 ||
        prctl(PR_GET_NO_NEW_PRIVS, 0UL, 0UL, 0UL, 0UL) != 1) {
        fputs("source build guard could not lock privilege gains\n", stderr);
        return 126;
    }
    execvp(argv[1], argv + 1);
    fputs("source build guard could not execute the command\n", stderr);
    return 126;
}
