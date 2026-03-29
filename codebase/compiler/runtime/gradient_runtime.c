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
#include <ctype.h>

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

/* ── Phase OO: HashMap type ────────────────────────────────────────────── */

/*
 * Map memory layout (heap-allocated struct):
 *
 *   typedef struct GradientMap {
 *       int64_t  size;        // number of entries currently stored
 *       int64_t  capacity;    // allocated slot count
 *       char**   keys;        // heap array of key strings (NULL = empty slot)
 *       int64_t* values;      // heap array of i64 values
 *                             //   For Map[String, String]: values[i] is a char*
 *                             //     cast to int64_t.
 *                             //   For Map[String, Int]:    values[i] is an i64.
 *   } GradientMap;
 *
 * The struct is 32 bytes on 64-bit platforms.
 * Codegen accesses these fields at byte offsets:
 *   offset  0: size     (i64)
 *   offset  8: capacity (i64)
 *   offset 16: keys ptr (i64 — pointer)
 *   offset 24: values ptr (i64 — pointer)
 *
 * We use a simple sorted-key linear-search strategy (O(n)) which is correct
 * for all map sizes encountered in practice.  A hash table upgrade is future
 * work.
 */

#define GRADIENT_MAP_INIT_CAP 8

typedef struct {
    int64_t  size;
    int64_t  capacity;
    char**   keys;
    int64_t* values;
} GradientMap;

static GradientMap* map_alloc(int64_t cap) {
    GradientMap* m = (GradientMap*)malloc(sizeof(GradientMap));
    m->size     = 0;
    m->capacity = cap;
    m->keys     = (char**)calloc((size_t)cap, sizeof(char*));
    m->values   = (int64_t*)calloc((size_t)cap, sizeof(int64_t));
    return m;
}

/*
 * __gradient_map_new() -> GradientMap*
 *
 * Allocate and return an empty map.
 */
void* __gradient_map_new(void) {
    return (void*)map_alloc(GRADIENT_MAP_INIT_CAP);
}

/* Internal: find index of key, returns -1 if absent. */
static int64_t map_find(GradientMap* m, const char* key) {
    for (int64_t i = 0; i < m->size; i++) {
        if (m->keys[i] && strcmp(m->keys[i], key) == 0)
            return i;
    }
    return -1;
}

/* Internal: grow the map arrays by 2x. */
static void map_grow(GradientMap* m) {
    int64_t new_cap = m->capacity * 2;
    m->keys   = (char**)realloc(m->keys,   (size_t)new_cap * sizeof(char*));
    m->values = (int64_t*)realloc(m->values, (size_t)new_cap * sizeof(int64_t));
    /* Zero out new slots. */
    for (int64_t i = m->capacity; i < new_cap; i++) {
        m->keys[i]   = NULL;
        m->values[i] = 0;
    }
    m->capacity = new_cap;
}

/* Internal: copy a map (shallow — values are copied as raw i64). */
static GradientMap* map_copy(GradientMap* src) {
    GradientMap* dst = (GradientMap*)malloc(sizeof(GradientMap));
    dst->size     = src->size;
    dst->capacity = src->capacity;
    dst->keys     = (char**)malloc((size_t)src->capacity * sizeof(char*));
    dst->values   = (int64_t*)malloc((size_t)src->capacity * sizeof(int64_t));
    for (int64_t i = 0; i < src->capacity; i++) {
        dst->keys[i]   = src->keys[i]   ? strdup(src->keys[i]) : NULL;
        dst->values[i] = src->values[i];
    }
    return dst;
}

/*
 * __gradient_map_set_str(map, key, value) -> GradientMap*
 *
 * Insert or update a Map[String, String] entry.  Returns the (possibly
 * reallocated) map pointer.
 * Gradient maps are persistent-by-copy: each set returns a new map.
 */
void* __gradient_map_set_str(void* map, const char* key, const char* value) {
    GradientMap* src = (GradientMap*)map;
    GradientMap* m   = map_copy(src);

    int64_t idx = map_find(m, key);
    if (idx >= 0) {
        /* Update existing entry. */
        free(m->keys[idx]);
        m->keys[idx]   = strdup(key);
        m->values[idx] = (int64_t)(intptr_t)strdup(value);
    } else {
        /* Insert new entry. */
        if (m->size >= m->capacity) map_grow(m);
        int64_t i = m->size++;
        m->keys[i]   = strdup(key);
        m->values[i] = (int64_t)(intptr_t)strdup(value);
    }
    return (void*)m;
}

/*
 * __gradient_map_set_int(map, key, value) -> GradientMap*
 *
 * Insert or update a Map[String, Int] entry.
 */
void* __gradient_map_set_int(void* map, const char* key, int64_t value) {
    GradientMap* src = (GradientMap*)map;
    GradientMap* m   = map_copy(src);

    int64_t idx = map_find(m, key);
    if (idx >= 0) {
        free(m->keys[idx]);
        m->keys[idx]   = strdup(key);
        m->values[idx] = value;
    } else {
        if (m->size >= m->capacity) map_grow(m);
        int64_t i = m->size++;
        m->keys[i]   = strdup(key);
        m->values[i] = value;
    }
    return (void*)m;
}

/*
 * __gradient_map_get_str(map, key) -> char* or NULL
 *
 * Look up a Map[String, String] entry.
 * Returns the value string pointer if found, NULL if absent.
 * Codegen wraps this in a Some/None Option construction.
 */
const char* __gradient_map_get_str(void* map, const char* key) {
    GradientMap* m = (GradientMap*)map;
    int64_t idx = map_find(m, key);
    if (idx < 0) return NULL;
    return (const char*)(intptr_t)m->values[idx];
}

/*
 * __gradient_map_get_int(map, key, found_out) -> int64_t
 *
 * Look up a Map[String, Int] entry.  Writes 1 to *found_out if the key
 * exists, 0 otherwise.  Returns 0 when not found.
 */
int64_t __gradient_map_get_int(void* map, const char* key, int64_t* found_out) {
    GradientMap* m = (GradientMap*)map;
    int64_t idx = map_find(m, key);
    if (idx < 0) { *found_out = 0; return 0; }
    *found_out = 1;
    return m->values[idx];
}

/*
 * __gradient_map_contains(map, key) -> int64_t (0 or 1)
 */
int64_t __gradient_map_contains(void* map, const char* key) {
    GradientMap* m = (GradientMap*)map;
    return map_find(m, key) >= 0 ? 1 : 0;
}

/*
 * __gradient_map_remove(map, key) -> GradientMap*
 *
 * Return a new map with the key removed (no-op if absent).
 */
void* __gradient_map_remove(void* map, const char* key) {
    GradientMap* src = (GradientMap*)map;
    GradientMap* m   = map_copy(src);

    int64_t idx = map_find(m, key);
    if (idx < 0) return (void*)m;  /* key not present, return copy as-is */

    /* Shift entries down to fill the gap. */
    free(m->keys[idx]);
    for (int64_t i = idx; i < m->size - 1; i++) {
        m->keys[i]   = m->keys[i + 1];
        m->values[i] = m->values[i + 1];
    }
    m->size--;
    m->keys[m->size]   = NULL;
    m->values[m->size] = 0;
    return (void*)m;
}

/*
 * __gradient_map_size(map) -> int64_t
 */
int64_t __gradient_map_size(void* map) {
    GradientMap* m = (GradientMap*)map;
    return m->size;
}

/*
 * __gradient_map_keys(map) -> List[String] (Gradient list pointer)
 *
 * Returns a Gradient list (layout: [size: i64, capacity: i64, data...])
 * where each element is a char* (string pointer) stored as an i64.
 */
void* __gradient_map_keys(void* map) {
    GradientMap* m   = (GradientMap*)map;
    int64_t n        = m->size;
    /* Gradient list: 16-byte header + n * 8 bytes data */
    void* list = malloc((size_t)(16 + n * 8));
    int64_t* hdr = (int64_t*)list;
    hdr[0] = n;   /* length    */
    hdr[1] = n;   /* capacity  */
    int64_t* data = hdr + 2;
    for (int64_t i = 0; i < n; i++) {
        data[i] = (int64_t)(intptr_t)strdup(m->keys[i]);
    }
    return list;
}

/*
 * __gradient_string_split(s, delim) -> List[String]
 *
 * Splits `s` on every occurrence of `delim` and returns a Gradient list of
 * the resulting substrings (layout: [size: i64, capacity: i64, data...]).
 * An empty `delim` returns a single-element list containing a copy of `s`.
 */
void* __gradient_string_split(const char* s, const char* delim) {
    if (!s) s = "";
    /* Count occurrences to pre-size the list. */
    size_t delim_len = delim ? strlen(delim) : 0;
    int64_t count = 0;
    if (delim_len == 0) {
        /* No delimiter: single element. */
        count = 1;
    } else {
        const char* p = s;
        count = 1;
        while ((p = strstr(p, delim)) != NULL) { count++; p += delim_len; }
    }
    /* Allocate list. */
    void* list = malloc((size_t)(16 + count * 8));
    int64_t* hdr  = (int64_t*)list;
    hdr[0] = count;  /* length   */
    hdr[1] = count;  /* capacity */
    int64_t* data = hdr + 2;
    if (delim_len == 0) {
        data[0] = (int64_t)(intptr_t)strdup(s);
        return list;
    }
    /* Fill list with split tokens. */
    const char* start = s;
    const char* found;
    int64_t idx = 0;
    while ((found = strstr(start, delim)) != NULL) {
        size_t len = (size_t)(found - start);
        char* tok = (char*)malloc(len + 1);
        memcpy(tok, start, len);
        tok[len] = '\0';
        data[idx++] = (int64_t)(intptr_t)tok;
        start = found + delim_len;
    }
    /* Last token (remainder after final delimiter). */
    data[idx] = (int64_t)(intptr_t)strdup(start);
    return list;
}

/*
 * __gradient_string_trim(s) -> char*
 * Returns a new heap-allocated string with leading and trailing whitespace removed.
 */
char* __gradient_string_trim(const char* s) {
    if (!s) return strdup("");
    const char* start = s;
    while (*start && isspace((unsigned char)*start)) start++;
    const char* end = s + strlen(s);
    while (end > start && isspace((unsigned char)*(end - 1))) end--;
    size_t len = (size_t)(end - start);
    char* result = (char*)malloc(len + 1);
    memcpy(result, start, len);
    result[len] = '\0';
    return result;
}
