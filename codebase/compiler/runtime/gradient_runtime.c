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

/*
 * map_destroy(map)
 *
 * Free a GradientMap and all its contents. Call this to release memory
 * when a map is no longer needed.
 *
 * Note: For String value maps, caller must ensure values are freed.
 * This version frees keys only (safe for all map types).
 */
void map_destroy(void* map) {
    GradientMap* m = (GradientMap*)map;
    if (!m) return;
    for (int64_t i = 0; i < m->size; i++) {
        if (m->keys[i]) free(m->keys[i]);
    }
    free(m->keys);
    free(m->values);
    free(m);
}

/*
 * map_destroy_str_values(map)
 *
 * Free a GradientMap AND all string values. Use this for Map[String, String].
 */
void map_destroy_str_values(void* map) {
    GradientMap* m = (GradientMap*)map;
    if (!m) return;
    for (int64_t i = 0; i < m->size; i++) {
        if (m->keys[i]) free(m->keys[i]);
        if (m->values[i]) {
            char* val = (char*)(intptr_t)m->values[i];
            free(val);
        }
    }
    free(m->keys);
    free(m->values);
    free(m);
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

/* ── Phase RR: HTTP Client Builtins (Net effect) ──────────────────────── */

/*
 * HTTP client implementation using libcurl.
 *
 * All HTTP functions return a Gradient Result[String, String] compatible
 * heap layout: [tag: i64, payload: char*]
 *   tag = 0 (Ok):  payload is the response body string
 *   tag = 1 (Err): payload is the error message string
 *
 * This matches the ConstructVariant layout so the compiler can directly
 * use the returned pointer as a Result enum value.
 */

#include <curl/curl.h>

/* Accumulator for curl write callback. */
typedef struct {
    char*  data;
    size_t size;
} CurlBuffer;

static size_t curl_write_cb(void* contents, size_t size, size_t nmemb, void* userp) {
    size_t total = size * nmemb;
    CurlBuffer* buf = (CurlBuffer*)userp;
    char* tmp = (char*)realloc(buf->data, buf->size + total + 1);
    if (!tmp) return 0;
    buf->data = tmp;
    memcpy(buf->data + buf->size, contents, total);
    buf->size += total;
    buf->data[buf->size] = '\0';
    return total;
}

/*
 * Build a Gradient Result[String, String] on the heap.
 * tag 0 = Ok(body), tag 1 = Err(msg).
 */
static void* make_result(int64_t tag, const char* payload) {
    int64_t* r = (int64_t*)malloc(16);
    r[0] = tag;
    r[1] = (int64_t)(intptr_t)strdup(payload ? payload : "");
    return (void*)r;
}

/*
 * __gradient_http_get(url) -> Result[String, String]
 *
 * Performs an HTTP GET request to the given URL.
 * Returns Ok(body) on success, Err(message) on failure.
 */
void* __gradient_http_get(const char* url) {
    if (!url || !*url) return make_result(1, "http_get: empty URL");

    CURL* curl = curl_easy_init();
    if (!curl) return make_result(1, "http_get: failed to initialize curl");

    CurlBuffer buf = { .data = (char*)malloc(1), .size = 0 };
    buf.data[0] = '\0';

    curl_easy_setopt(curl, CURLOPT_URL, url);
    curl_easy_setopt(curl, CURLOPT_WRITEFUNCTION, curl_write_cb);
    curl_easy_setopt(curl, CURLOPT_WRITEDATA, &buf);
    curl_easy_setopt(curl, CURLOPT_FOLLOWLOCATION, 1L);
    curl_easy_setopt(curl, CURLOPT_TIMEOUT, 30L);

    CURLcode res = curl_easy_perform(curl);
    curl_easy_cleanup(curl);

    if (res != CURLE_OK) {
        free(buf.data);
        const char* err = curl_easy_strerror(res);
        char msg[256];
        snprintf(msg, sizeof(msg), "http_get: %s", err);
        return make_result(1, msg);
    }

    void* result = make_result(0, buf.data);
    free(buf.data);
    return result;
}

/*
 * Internal: perform an HTTP POST with the given content type.
 */
static void* http_post_impl(const char* url, const char* body,
                             const char* content_type) {
    if (!url || !*url) return make_result(1, "http_post: empty URL");

    CURL* curl = curl_easy_init();
    if (!curl) return make_result(1, "http_post: failed to initialize curl");

    CurlBuffer buf = { .data = (char*)malloc(1), .size = 0 };
    buf.data[0] = '\0';

    struct curl_slist* headers = NULL;
    if (content_type) {
        char hdr[128];
        snprintf(hdr, sizeof(hdr), "Content-Type: %s", content_type);
        headers = curl_slist_append(headers, hdr);
        curl_easy_setopt(curl, CURLOPT_HTTPHEADER, headers);
    }

    curl_easy_setopt(curl, CURLOPT_URL, url);
    curl_easy_setopt(curl, CURLOPT_POSTFIELDS, body ? body : "");
    curl_easy_setopt(curl, CURLOPT_WRITEFUNCTION, curl_write_cb);
    curl_easy_setopt(curl, CURLOPT_WRITEDATA, &buf);
    curl_easy_setopt(curl, CURLOPT_FOLLOWLOCATION, 1L);
    curl_easy_setopt(curl, CURLOPT_TIMEOUT, 30L);

    CURLcode res = curl_easy_perform(curl);
    if (headers) curl_slist_free_all(headers);
    curl_easy_cleanup(curl);

    if (res != CURLE_OK) {
        free(buf.data);
        const char* err = curl_easy_strerror(res);
        char msg[256];
        snprintf(msg, sizeof(msg), "http_post: %s", err);
        return make_result(1, msg);
    }

    void* result = make_result(0, buf.data);
    free(buf.data);
    return result;
}

/*
 * __gradient_http_post(url, body) -> Result[String, String]
 *
 * Performs an HTTP POST request with the given body.
 * Returns Ok(response_body) on success, Err(message) on failure.
 */
void* __gradient_http_post(const char* url, const char* body) {
    return http_post_impl(url, body, NULL);
}

/*
 * __gradient_http_post_json(url, json) -> Result[String, String]
 *
 * Performs an HTTP POST request with Content-Type: application/json.
 * Returns Ok(response_body) on success, Err(message) on failure.
 */
void* __gradient_http_post_json(const char* url, const char* json) {
    return http_post_impl(url, json, "application/json");
}

/* ── Phase PP: Design-by-Contract Runtime Support ────────────────────────── */

/*
 * __gradient_contract_fail(message)
 *
 * Called when a contract (@requires or @ensures) is violated.
 * Prints a structured error message to stderr and aborts the program.
 * This function never returns.
 */
void __gradient_contract_fail(const char* message) {
    fprintf(stderr, "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");
    fprintf(stderr, "  CONTRACT VIOLATION\n");
    fprintf(stderr, "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");
    fprintf(stderr, "  %s\n", message);
    fprintf(stderr, "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");
    fflush(stderr);
    abort();
}
