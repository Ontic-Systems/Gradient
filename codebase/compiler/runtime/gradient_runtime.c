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
 *   Program arguments:
 *     __gradient_save_args   -- called by main to save argc/argv
 *     __gradient_get_args    -- args() -> !{IO} List[String]
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
#include <stdarg.h>

/* ── Program arguments ─────────────────────────────────────────────────── */

static int    __gradient_saved_argc = 0;
static char** __gradient_saved_argv = NULL;

/*
 * __gradient_save_args(argc, argv)
 *
 * Called at the start of main to save argc/argv for later retrieval
 * by the args() builtin.
 */
void __gradient_save_args(int64_t argc, char** argv) {
    __gradient_saved_argc = (int)argc;
    __gradient_saved_argv = argv;
}

/*
 * __gradient_get_args() -> List[String]
 *
 * Returns a Gradient list (layout: [size: i64, capacity: i64, data...])
 * where each element is a strdup'd copy of the corresponding argv entry.
 */
void* __gradient_get_args(void) {
    int64_t n = (int64_t)__gradient_saved_argc;
    void* list = malloc((size_t)(16 + n * 8));
    int64_t* hdr = (int64_t*)list;
    hdr[0] = n;   /* length   */
    hdr[1] = n;   /* capacity */
    int64_t* data = hdr + 2;
    for (int64_t i = 0; i < n; i++) {
        data[i] = (int64_t)(intptr_t)strdup(__gradient_saved_argv[i]);
    }
    return list;
}

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

/* ── Phase PP: JSON Parser/Serializer ──────────────────────────────────── */

#define JSON_NULL   0
#define JSON_BOOL   1
#define JSON_INT    2
#define JSON_FLOAT  3
#define JSON_STRING 4
#define JSON_ARRAY  5
#define JSON_OBJECT 6

static void* json_alloc(int64_t tag, int n_payload) {
    int64_t* ptr = (int64_t*)malloc((size_t)(8 + n_payload * 8));
    if (!ptr) return NULL;
    ptr[0] = tag;
    return (void*)ptr;
}

static void* json_null(void) {
    return json_alloc(JSON_NULL, 0);
}

static void* json_bool(int64_t val) {
    int64_t* p = (int64_t*)json_alloc(JSON_BOOL, 1);
    if (!p) return NULL;
    p[1] = val ? 1 : 0;
    return (void*)p;
}

static void* json_int(int64_t val) {
    int64_t* p = (int64_t*)json_alloc(JSON_INT, 1);
    if (!p) return NULL;
    p[1] = val;
    return (void*)p;
}

static void* json_float(double val) {
    int64_t* p = (int64_t*)json_alloc(JSON_FLOAT, 1);
    if (!p) return NULL;
    memcpy(&p[1], &val, sizeof(double));
    return (void*)p;
}

static void* json_string_owned(char* s) {
    int64_t* p = (int64_t*)json_alloc(JSON_STRING, 1);
    if (!p) {
        free(s);
        return NULL;
    }
    p[1] = (int64_t)(intptr_t)s;
    return (void*)p;
}

static void* json_string(const char* s) {
    return json_string_owned(strdup(s ? s : ""));
}

static void* json_array(void* list_ptr) {
    int64_t* p = (int64_t*)json_alloc(JSON_ARRAY, 1);
    if (!p) return NULL;
    p[1] = (int64_t)(intptr_t)list_ptr;
    return (void*)p;
}

static void* json_object(void* map_ptr) {
    int64_t* p = (int64_t*)json_alloc(JSON_OBJECT, 1);
    if (!p) return NULL;
    p[1] = (int64_t)(intptr_t)map_ptr;
    return (void*)p;
}

typedef struct {
    const char* input;
    size_t pos;
    char error[256];
} JsonParser;

static void json_set_error(JsonParser* p, const char* fmt, ...) {
    va_list args;
    va_start(args, fmt);
    vsnprintf(p->error, sizeof(p->error), fmt, args);
    va_end(args);
}

static void json_skip_ws(JsonParser* p) {
    while (p->input[p->pos] && strchr(" \t\n\r", p->input[p->pos])) {
        p->pos++;
    }
}

static void json_free_value(void* val);
static void* json_parse_value(JsonParser* p);

static char* json_parse_string_raw(JsonParser* p) {
    if (p->input[p->pos] != '"') {
        json_set_error(p, "expected '\"' at pos %zu", p->pos);
        return NULL;
    }
    p->pos++;

    size_t cap = 32;
    size_t len = 0;
    char* buf = (char*)malloc(cap);
    if (!buf) {
        json_set_error(p, "out of memory while parsing string");
        return NULL;
    }

    while (p->input[p->pos] && p->input[p->pos] != '"') {
        char ch = p->input[p->pos++];
        if (ch == '\\') {
            char esc = p->input[p->pos++];
            switch (esc) {
                case '"': ch = '"'; break;
                case '\\': ch = '\\'; break;
                case '/': ch = '/'; break;
                case 'b': ch = '\b'; break;
                case 'f': ch = '\f'; break;
                case 'n': ch = '\n'; break;
                case 'r': ch = '\r'; break;
                case 't': ch = '\t'; break;
                case '\0':
                    free(buf);
                    json_set_error(p, "unterminated escape at pos %zu", p->pos);
                    return NULL;
                default:
                    free(buf);
                    json_set_error(p, "unsupported escape '\\%c' at pos %zu", esc, p->pos - 1);
                    return NULL;
            }
        }

        if (len + 2 > cap) {
            cap *= 2;
            char* tmp = (char*)realloc(buf, cap);
            if (!tmp) {
                free(buf);
                json_set_error(p, "out of memory while growing string buffer");
                return NULL;
            }
            buf = tmp;
        }
        buf[len++] = ch;
    }

    if (p->input[p->pos] != '"') {
        free(buf);
        json_set_error(p, "unterminated string at pos %zu", p->pos);
        return NULL;
    }

    p->pos++;
    buf[len] = '\0';
    return buf;
}

static void* json_parse_number(JsonParser* p) {
    const char* start = p->input + p->pos;
    char* end = NULL;
    double f = strtod(start, &end);
    if (end == start) {
        json_set_error(p, "invalid number at pos %zu", p->pos);
        return NULL;
    }

    int is_float = 0;
    for (const char* cur = start; cur < end; cur++) {
        if (*cur == '.' || *cur == 'e' || *cur == 'E') {
            is_float = 1;
            break;
        }
    }

    p->pos += (size_t)(end - start);
    if (is_float) {
        return json_float(f);
    }
    return json_int((int64_t)f);
}

static void* json_parse_array(JsonParser* p) {
    p->pos++;
    json_skip_ws(p);

    int64_t cap = 8;
    int64_t len = 0;
    int64_t* items = (int64_t*)malloc((size_t)(cap * 8));
    if (!items) {
        json_set_error(p, "out of memory while parsing array");
        return NULL;
    }

    if (p->input[p->pos] != ']') {
        while (1) {
            json_skip_ws(p);
            void* val = json_parse_value(p);
            if (!val) {
                for (int64_t i = 0; i < len; i++) {
                    json_free_value((void*)(intptr_t)items[i]);
                }
                free(items);
                return NULL;
            }

            if (len >= cap) {
                cap *= 2;
                int64_t* tmp = (int64_t*)realloc(items, (size_t)(cap * 8));
                if (!tmp) {
                    json_free_value(val);
                    for (int64_t i = 0; i < len; i++) {
                        json_free_value((void*)(intptr_t)items[i]);
                    }
                    free(items);
                    json_set_error(p, "out of memory while growing array");
                    return NULL;
                }
                items = tmp;
            }
            items[len++] = (int64_t)(intptr_t)val;

            json_skip_ws(p);
            if (p->input[p->pos] == ',') {
                p->pos++;
                continue;
            }
            if (p->input[p->pos] == ']') break;

            for (int64_t i = 0; i < len; i++) {
                json_free_value((void*)(intptr_t)items[i]);
            }
            free(items);
            json_set_error(p, "expected ',' or ']' at pos %zu", p->pos);
            return NULL;
        }
    }

    p->pos++;
    int64_t* list = (int64_t*)malloc((size_t)(16 + len * 8));
    if (!list) {
        for (int64_t i = 0; i < len; i++) {
            json_free_value((void*)(intptr_t)items[i]);
        }
        free(items);
        json_set_error(p, "out of memory while finalizing array");
        return NULL;
    }
    list[0] = len;
    list[1] = len;
    memcpy(list + 2, items, (size_t)(len * 8));
    free(items);
    return json_array((void*)list);
}

static void* json_parse_object(JsonParser* p) {
    p->pos++;
    json_skip_ws(p);

    GradientMap* map = (GradientMap*)__gradient_map_new();
    if (!map) {
        json_set_error(p, "out of memory while parsing object");
        return NULL;
    }

    if (p->input[p->pos] != '}') {
        while (1) {
            json_skip_ws(p);
            char* key = json_parse_string_raw(p);
            if (!key) {
                map_destroy(map);
                return NULL;
            }

            json_skip_ws(p);
            if (p->input[p->pos] != ':') {
                free(key);
                map_destroy(map);
                json_set_error(p, "expected ':' at pos %zu", p->pos);
                return NULL;
            }
            p->pos++;

            json_skip_ws(p);
            void* val = json_parse_value(p);
            if (!val) {
                free(key);
                map_destroy(map);
                return NULL;
            }

            if (map->size >= map->capacity) map_grow(map);
            map->keys[map->size] = key;
            map->values[map->size] = (int64_t)(intptr_t)val;
            map->size++;

            json_skip_ws(p);
            if (p->input[p->pos] == ',') {
                p->pos++;
                continue;
            }
            if (p->input[p->pos] == '}') break;

            map_destroy(map);
            json_set_error(p, "expected ',' or '}' at pos %zu", p->pos);
            return NULL;
        }
    }

    p->pos++;
    return json_object((void*)map);
}

static void* json_parse_value(JsonParser* p) {
    json_skip_ws(p);
    char c = p->input[p->pos];

    if (c == '"') {
        char* s = json_parse_string_raw(p);
        if (!s) return NULL;
        return json_string_owned(s);
    }
    if (c == '[') return json_parse_array(p);
    if (c == '{') return json_parse_object(p);
    if (c == 't' && strncmp(p->input + p->pos, "true", 4) == 0) {
        p->pos += 4;
        return json_bool(1);
    }
    if (c == 'f' && strncmp(p->input + p->pos, "false", 5) == 0) {
        p->pos += 5;
        return json_bool(0);
    }
    if (c == 'n' && strncmp(p->input + p->pos, "null", 4) == 0) {
        p->pos += 4;
        return json_null();
    }
    if (c == '-' || (c >= '0' && c <= '9')) {
        return json_parse_number(p);
    }

    json_set_error(p, "unexpected char '%c' at pos %zu", c ? c : '?', p->pos);
    return NULL;
}

void* __gradient_json_parse(const char* input, int64_t* out_ok) {
    JsonParser parser = { .input = input ? input : "", .pos = 0, .error = {0} };
    void* result = json_parse_value(&parser);
    if (!result || parser.error[0]) {
        *out_ok = 0;
        if (result) json_free_value(result);
        return (void*)(intptr_t)strdup(parser.error[0] ? parser.error : "parse error");
    }

    json_skip_ws(&parser);
    if (parser.input[parser.pos] != '\0') {
        *out_ok = 0;
        json_free_value(result);
        json_set_error(&parser, "unexpected trailing input at pos %zu", parser.pos);
        return (void*)(intptr_t)strdup(parser.error);
    }

    *out_ok = 1;
    return result;
}

static void json_buf_append(char** buf, size_t* len, size_t* cap, const char* s) {
    size_t slen = strlen(s);
    while (*len + slen + 1 > *cap) {
        *cap *= 2;
        *buf = (char*)realloc(*buf, *cap);
    }
    memcpy(*buf + *len, s, slen);
    *len += slen;
    (*buf)[*len] = '\0';
}

static void json_stringify_string(const char* s, char** buf, size_t* len, size_t* cap) {
    json_buf_append(buf, len, cap, "\"");
    for (const char* p = s; *p; p++) {
        switch (*p) {
            case '"': json_buf_append(buf, len, cap, "\\\""); break;
            case '\\': json_buf_append(buf, len, cap, "\\\\"); break;
            case '\n': json_buf_append(buf, len, cap, "\\n"); break;
            case '\r': json_buf_append(buf, len, cap, "\\r"); break;
            case '\t': json_buf_append(buf, len, cap, "\\t"); break;
            default: {
                char tmp[2] = {*p, '\0'};
                json_buf_append(buf, len, cap, tmp);
                break;
            }
        }
    }
    json_buf_append(buf, len, cap, "\"");
}

static void json_stringify_value(void* val, char** buf, size_t* len, size_t* cap) {
    int64_t tag = ((int64_t*)val)[0];
    switch (tag) {
        case JSON_NULL:
            json_buf_append(buf, len, cap, "null");
            break;
        case JSON_BOOL:
            json_buf_append(buf, len, cap, ((int64_t*)val)[1] ? "true" : "false");
            break;
        case JSON_INT: {
            char tmp[32];
            snprintf(tmp, sizeof(tmp), "%lld", (long long)((int64_t*)val)[1]);
            json_buf_append(buf, len, cap, tmp);
            break;
        }
        case JSON_FLOAT: {
            double f;
            memcpy(&f, &((int64_t*)val)[1], sizeof(double));
            char tmp[64];
            snprintf(tmp, sizeof(tmp), "%g", f);
            json_buf_append(buf, len, cap, tmp);
            break;
        }
        case JSON_STRING: {
            const char* s = (const char*)(intptr_t)((int64_t*)val)[1];
            json_stringify_string(s ? s : "", buf, len, cap);
            break;
        }
        case JSON_ARRAY: {
            int64_t* list = (int64_t*)(intptr_t)((int64_t*)val)[1];
            int64_t count = list[0];
            int64_t* data = list + 2;
            json_buf_append(buf, len, cap, "[");
            for (int64_t i = 0; i < count; i++) {
                if (i > 0) json_buf_append(buf, len, cap, ",");
                json_stringify_value((void*)(intptr_t)data[i], buf, len, cap);
            }
            json_buf_append(buf, len, cap, "]");
            break;
        }
        case JSON_OBJECT: {
            GradientMap* m = (GradientMap*)(intptr_t)((int64_t*)val)[1];
            json_buf_append(buf, len, cap, "{");
            for (int64_t i = 0; i < m->size; i++) {
                if (i > 0) json_buf_append(buf, len, cap, ",");
                json_stringify_string(m->keys[i], buf, len, cap);
                json_buf_append(buf, len, cap, ":");
                json_stringify_value((void*)(intptr_t)m->values[i], buf, len, cap);
            }
            json_buf_append(buf, len, cap, "}");
            break;
        }
        default:
            json_buf_append(buf, len, cap, "null");
            break;
    }
}

char* __gradient_json_stringify(void* val) {
    size_t cap = 256;
    size_t len = 0;
    char* buf = (char*)malloc(cap);
    if (!buf) return strdup("null");
    buf[0] = '\0';
    json_stringify_value(val, &buf, &len, &cap);
    return buf;
}

char* __gradient_json_type(void* val) {
    if (!val) return strdup("null");
    switch (((int64_t*)val)[0]) {
        case JSON_NULL: return strdup("null");
        case JSON_BOOL: return strdup("bool");
        case JSON_INT: return strdup("int");
        case JSON_FLOAT: return strdup("float");
        case JSON_STRING: return strdup("string");
        case JSON_ARRAY: return strdup("array");
        case JSON_OBJECT: return strdup("object");
        default: return strdup("unknown");
    }
}

int64_t __gradient_json_is_null(void* val) {
    return (val && ((int64_t*)val)[0] == JSON_NULL) ? 1 : 0;
}

void* __gradient_json_get(void* val, const char* key) {
    if (!val || !key) return NULL;
    if (((int64_t*)val)[0] != JSON_OBJECT) return NULL;
    GradientMap* m = (GradientMap*)(intptr_t)((int64_t*)val)[1];
    int64_t idx = map_find(m, key);
    if (idx < 0) return NULL;
    return (void*)(intptr_t)m->values[idx];
}

static void json_free_value(void* val) {
    if (!val) return;
    int64_t tag = ((int64_t*)val)[0];
    switch (tag) {
        case JSON_STRING:
            free((void*)(intptr_t)((int64_t*)val)[1]);
            break;
        case JSON_ARRAY: {
            int64_t* list = (int64_t*)(intptr_t)((int64_t*)val)[1];
            int64_t count = list[0];
            int64_t* data = list + 2;
            for (int64_t i = 0; i < count; i++) {
                json_free_value((void*)(intptr_t)data[i]);
            }
            free(list);
            break;
        }
        case JSON_OBJECT: {
            GradientMap* m = (GradientMap*)(intptr_t)((int64_t*)val)[1];
            for (int64_t i = 0; i < m->size; i++) {
                free(m->keys[i]);
                json_free_value((void*)(intptr_t)m->values[i]);
            }
            free(m->keys);
            free(m->values);
            free(m);
            break;
        }
        default:
            break;
    }
    free(val);
}
