/*
 * Gradient Runtime Helpers
 *
 * Consolidated C-side helpers for all Gradient builtins that require libc
 * support beyond what can be declared as direct libc imports.
 *
 * This file is compiled and linked automatically by `gradient build`.
 * To link manually:
 *   cc -c runtime/gradient_runtime.c -o gradient_runtime.o
 *   cc your_program.o gradient_runtime.o -o your_program
 *
 * Functions defined here:
 *
 *   Phase MM — Standard I/O:
 *     __gradient_read_line   -- read_line() -> !{IO} String
 *
 *   Phase NN — File I/O (FS effect):
 *     __gradient_file_read   -- file_read(path: String) -> !{FS} String
 *     __gradient_file_write  -- file_write(path: String, content: String) -> !{FS} Bool
 *     __gradient_file_exists -- file_exists(path: String) -> !{FS} Bool
 *     __gradient_file_append -- file_append(path: String, content: String) -> !{FS} Bool
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>
#include <unistd.h>

/* ── Phase MM: Standard I/O ─────────────────────────────────────────────── */

/*
 * __gradient_read_line() -> char*
 *
 * Reads one line from stdin using getline(). Strips the trailing newline
 * character if present. Returns a malloc'd string that the caller owns
 * (and should free when done). Returns an empty malloc'd string on EOF or
 * error.
 */
char* __gradient_read_line(void) {
    char* line = NULL;
    size_t len = 0;
    ssize_t nread = getline(&line, &len, stdin);
    if (nread == -1) {
        /* EOF or error: return empty string */
        if (line) free(line);
        return strdup("");
    }
    /* Strip trailing newline */
    if (nread > 0 && line[nread - 1] == '\n') {
        line[nread - 1] = '\0';
    }
    return line;
}

/* ── Phase NN: File I/O (FS effect) ─────────────────────────────────────── */

/*
 * __gradient_file_read(path) -> char*
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

/*
 * __gradient_file_write(path, content) -> int64_t
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

/*
 * __gradient_file_exists(path) -> int64_t
 *
 * Returns 1 (true) if the file at `path` exists and is accessible,
 * 0 (false) otherwise.  Uses POSIX access(2) with F_OK.
 */
int64_t __gradient_file_exists(const char* path) {
    return access(path, F_OK) == 0 ? 1 : 0;
}

/*
 * __gradient_file_append(path, content) -> int64_t
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
