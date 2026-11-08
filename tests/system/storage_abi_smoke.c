#include "open_desktop/storage_abi_v1.h"

#include <assert.h>
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

_Static_assert(sizeof(od_storage_error_v1) == OD_STORAGE_ERROR_SIZE_V1, "od_storage_error_v1 layout changed");
_Static_assert(sizeof(od_storage_image_info_v1) == OD_STORAGE_IMAGE_INFO_SIZE_V1,
               "od_storage_image_info_v1 layout changed");

static uint32_t create(int dir, const char *name, uint64_t capacity, od_storage_error_v1 *error) {
    memset(error, 0, sizeof(*error));
    error->struct_size = OD_STORAGE_ERROR_SIZE_V1;
    return od_storage_create_raw_v1(dir, (const uint8_t *)name, (uint32_t)strlen(name), capacity, 0, error);
}

static uint32_t inspect(int dir, const char *name, uint64_t expected, od_storage_image_info_v1 *info) {
    memset(info, 0, sizeof(*info));
    info->struct_size = OD_STORAGE_IMAGE_INFO_SIZE_V1;
    info->expected_size_bytes = expected;
    return od_storage_inspect_v1(dir, (const uint8_t *)name, (uint32_t)strlen(name), info);
}

int main(void) {
    char base[] = "/tmp/od_storage_abi_smoke.XXXXXX";
    char path[1024];
    od_storage_error_v1 error;
    od_storage_image_info_v1 info;
    struct stat st;

    assert(od_storage_abi_version() == OD_STORAGE_ABI_VERSION_V1);
    assert(mkdtemp(base) != NULL);
    snprintf(path, sizeof(path), "%s/disks", base);
    assert(mkdir(path, 0700) == 0);
    snprintf(path, sizeof(path), "%s/outside", base);
    assert(mkdir(path, 0700) == 0);
    snprintf(path, sizeof(path), "%s/disks", base);
    int dir = open(path, O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC);
    assert(dir >= 0);

    /* Create: exact size, mode 0600, regular, zero bytes. */
    assert(create(dir, "disk0.img", 8192, &error) == OD_STORAGE_STATUS_OK_V1);
    assert(error.struct_size == OD_STORAGE_ERROR_SIZE_V1 && error.os_errno == 0);
    assert(fstatat(dir, "disk0.img", &st, AT_SYMLINK_NOFOLLOW) == 0);
    assert(S_ISREG(st.st_mode) && st.st_size == 8192 && (st.st_mode & 0777) == 0600);
    int fd = openat(dir, "disk0.img", O_RDONLY | O_NOFOLLOW);
    assert(fd >= 0);
    unsigned char buf[8192];
    assert(read(fd, buf, sizeof(buf)) == (ssize_t)sizeof(buf));
    for (size_t i = 0; i < sizeof(buf); i++) assert(buf[i] == 0);
    close(fd);

    /* Existing name: refused, content untouched. */
    fd = openat(dir, "disk0.img", O_WRONLY | O_NOFOLLOW);
    assert(fd >= 0 && write(fd, "X", 1) == 1);
    close(fd);
    assert(create(dir, "disk0.img", 4096, &error) == OD_STORAGE_STATUS_ALREADY_EXISTS_V1);
    assert(error.os_errno == EEXIST);
    assert(fstatat(dir, "disk0.img", &st, AT_SYMLINK_NOFOLLOW) == 0 && st.st_size == 8192);

    /* Names: refused, nothing created anywhere. */
    const char *bad[] = {"", ".", "..", "a/b", "../escape", "/abs", "ctl\n", "del\x7f"};
    for (size_t i = 0; i < sizeof(bad) / sizeof(bad[0]); i++) {
        assert(create(dir, bad[i], 4096, &error) == OD_STORAGE_STATUS_INVALID_NAME_V1);
        assert(inspect(dir, bad[i], 0, &info) == OD_STORAGE_STATUS_INVALID_NAME_V1);
    }
    assert(od_storage_create_raw_v1(dir, (const uint8_t *)"a\0b", 3, 4096, 0, &error) ==
           OD_STORAGE_STATUS_INVALID_NAME_V1);
    snprintf(path, sizeof(path), "%s/escape", base);
    assert(stat(path, &st) != 0);

    /* Symlink in place of an image: refused, never followed. */
    snprintf(path, sizeof(path), "%s/outside/target", base);
    assert(symlinkat(path, dir, "link.img") == 0);
    assert(create(dir, "link.img", 4096, &error) == OD_STORAGE_STATUS_ALREADY_EXISTS_V1);
    assert(stat(path, &st) != 0); /* target was not created through the link */
    assert(inspect(dir, "link.img", 0, &info) == OD_STORAGE_STATUS_NOT_REGULAR_FILE_V1);

    /* Not enough space: a capacity no volume has, numbers reported, nothing created. */
    uint64_t huge = UINT64_C(0x7FFFFFFFFFFFFE00);
    assert(create(dir, "huge.img", huge, &error) == OD_STORAGE_STATUS_INSUFFICIENT_SPACE_V1);
    assert(error.required_bytes == huge && error.available_bytes < huge);
    assert(fstatat(dir, "huge.img", &st, AT_SYMLINK_NOFOLLOW) != 0 && errno == ENOENT);

    /* Argument errors. */
    assert(create(dir, "z.img", 100, &error) == OD_STORAGE_STATUS_INVALID_ARGUMENT_V1);
    assert(create(dir, "z.img", 0, &error) == OD_STORAGE_STATUS_INVALID_ARGUMENT_V1);
    assert(od_storage_create_raw_v1(dir, (const uint8_t *)"z.img", 5, 4096, 1, &error) ==
           OD_STORAGE_STATUS_INVALID_ARGUMENT_V1);
    assert(create(-1, "z.img", 4096, &error) == OD_STORAGE_STATUS_INVALID_ARGUMENT_V1);

    /* Inspect: valid, truncated, oversized, malformed, missing. */
    assert(inspect(dir, "disk0.img", 8192, &info) == OD_STORAGE_STATUS_OK_V1);
    assert(info.format == OD_STORAGE_FORMAT_RAW_V1 && info.size_bytes == 8192);
    fd = openat(dir, "disk0.img", O_WRONLY | O_NOFOLLOW);
    assert(fd >= 0 && ftruncate(fd, 4096) == 0);
    assert(inspect(dir, "disk0.img", 8192, &info) == OD_STORAGE_STATUS_TRUNCATED_V1);
    assert(info.size_bytes == 4096 && info.format == 0);
    assert(inspect(dir, "disk0.img", 2048, &info) == OD_STORAGE_STATUS_SIZE_MISMATCH_V1);
    assert(info.size_bytes == 4096);
    assert(ftruncate(fd, 4000) == 0);
    assert(inspect(dir, "disk0.img", 0, &info) == OD_STORAGE_STATUS_INVALID_IMAGE_V1);
    assert(info.size_bytes == 4000);
    assert(ftruncate(fd, 0) == 0);
    assert(inspect(dir, "disk0.img", 0, &info) == OD_STORAGE_STATUS_INVALID_IMAGE_V1);
    close(fd);
    assert(inspect(dir, "missing.img", 0, &info) == OD_STORAGE_STATUS_NOT_FOUND_V1);
    assert(info.os_errno == ENOENT);

    close(dir);
    snprintf(path, sizeof(path), "rm -rf '%s'", base);
    assert(system(path) == 0);
    return 0;
}
