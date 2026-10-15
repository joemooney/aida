#define _GNU_SOURCE
#include <dirent.h>
#include <dlfcn.h>
#include <errno.h>
#include <limits.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
/* trace:BUG-1807 | ai:codex
 * Fixture only: fail iteration after an entry was successfully returned from
 * the selected directory. Exact private path; no production injection hook. */
struct dirent64 *readdir64(DIR *dir) {
    static int seen;
    struct dirent64 *(*real_fn)(DIR *) = dlsym(RTLD_NEXT, "readdir64");
    const char *target = getenv("BUG1807_READDIR_FAULT_PATH");
    char fdpath[64], path[PATH_MAX];
    snprintf(fdpath, sizeof(fdpath), "/proc/self/fd/%d", dirfd(dir));
    ssize_t n = readlink(fdpath, path, sizeof(path) - 1);
    if (n >= 0) path[n] = '\0';
    if (target && n >= 0 && strcmp(target, path) == 0) {
        if (seen++) {
            unsetenv("BUG1807_READDIR_FAULT_PATH");
            fprintf(stderr, "BUG1807 FIXTURE: injected EIO readdir64 after entry (%s)\n", path);
            errno = EIO;
            return NULL;
        }
    }
    return real_fn(dir);
}
