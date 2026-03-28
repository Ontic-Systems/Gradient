/**
 * Gradient Language Runtime Helpers
 *
 * This file implements the C-side helpers for Gradient builtins that require
 * libc support beyond what can be declared as direct libc imports.
 *
 * Compile alongside your Gradient object file:
 *   cc program.o runtime.c -o program
 *
 * Functions defined here:
 *   __gradient_file_read   -- file_read(path: String) -> !{FS} String
 *   __gradient_file_write  -- file_write(path: String, content: String) -> !{FS} Bool
 *   __gradient_file_exists -- file_exists(path: String) -> !{FS} Bool
 *   __gradient_file_append -- file_append(path: String, content: String) -> !{FS} Bool
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>
#include <unistd.h>

/**
 * file_read(path: String) -> !{FS} String
 *
 * Reads the entire contents of the file at `path` and returns a
 * heap-allocated, null-terminated string.  Returns an empty string
 * (not NULL) on any error so callers never have to handle NULL.
 */
char* __gradient_file_read(const char* path) {
    FILE* f = fopen(path, "r");
    if (!f) return strdup("");

    fseek(f, 0, SEEK_END);
    long size = ftell(f);
    rewind(f);

    char* buf = (char*)malloc(size + 1);
    if (!buf) {
        fclose(f);
        return strdup("");
    }

    size_t n = fread(buf, 1, (size_t)size, f);
    buf[n] = '\0';
    fclose(f);
    return buf;
}

/**
 * file_write(path: String, content: String) -> !{FS} Bool
 *
 * Creates or overwrites the file at `path` with `content`.
 * Returns 1 (true) on success, 0 (false) on failure.
 */
int64_t __gradient_file_write(const char* path, const char* content) {
    FILE* f = fopen(path, "w");
    if (!f) return 0;
    fputs(content, f);
    fclose(f);
    return 1;
}

/**
 * file_exists(path: String) -> !{FS} Bool
 *
 * Returns 1 (true) if the file at `path` exists and is accessible,
 * 0 (false) otherwise.  Uses POSIX access(2) with F_OK.
 */
int64_t __gradient_file_exists(const char* path) {
    return access(path, F_OK) == 0 ? 1 : 0;
}

/**
 * file_append(path: String, content: String) -> !{FS} Bool
 *
 * Appends `content` to the file at `path`, creating it if it does not
 * exist.  Returns 1 (true) on success, 0 (false) on failure.
 */
int64_t __gradient_file_append(const char* path, const char* content) {
    FILE* f = fopen(path, "a");
    if (!f) return 0;
    fputs(content, f);
    fclose(f);
    return 1;
}
