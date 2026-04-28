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
 *     __gradient_file_delete -- file_delete(path: String) -> !{FS} Bool
 *
 *   Phase PP — Random Number Generation:
 *     __gradient_random      -- random() -> Float
 *     __gradient_random_int  -- random_int(min: Int, max: Int) -> Int
 *     __gradient_random_float -- random_float() -> Float
 *     __gradient_seed_random -- seed_random(seed: Int) -> ()
 *
 *   Phase PP — Environment/Process (IO/Time effects):
 *     __gradient_get_env     -- get_env(name: String) -> !{IO} Option[String]
 *     __gradient_set_env     -- set_env(name: String, value: String) -> !{IO} ()
 *     __gradient_current_dir -- current_dir() -> !{IO} String
 *     __gradient_change_dir  -- change_dir(path: String) -> !{IO} ()
 *     __gradient_spawn       -- spawn(program: String, args: List[String]) -> !{IO} Int (M-5)
 *     getpid                 -- process_id() -> Int (pure)
 *     sleep                  -- sleep_seconds(s: Int) -> !{Time} ()
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>
#include <unistd.h>
#include <ctype.h>
#include <stdarg.h>
#include <errno.h>
#include <time.h>
#include <limits.h>

/* M-5: Headers for spawn() implementation */
#include <sys/types.h>
#include <sys/wait.h>
#ifdef _POSIX_PRIORITY_SCHEDULING
#include <spawn.h>
#endif
extern char** environ;  /* For posix_spawnp */

/* ── H-3: safe_realloc ──────────────────────────────────────────────────── */

/*
 * safe_realloc(ptr, size) -> void*
 *
 * Wraps realloc: if the allocation fails, frees the original pointer and
 * calls abort() so callers never receive a NULL without a stack trace.
 * Use this everywhere a failed realloc would otherwise leave dangling
 * memory or cause a use-after-free.
 */
static void* safe_realloc(void* ptr, size_t size) {
    void* result = realloc(ptr, size);
    if (!result) {
        free(ptr);
        abort();
    }
    return result;
}

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

    // SECURITY: Check for integer overflow before malloc
    // n * 8 could overflow if n is close to INT64_MAX
    if (n < 0 || n > (int64_t)((SIZE_MAX - 16) / 8)) {
        // Return empty list on overflow risk
        void* empty_list = malloc(16);
        if (!empty_list) return NULL;
        int64_t* hdr = (int64_t*)empty_list;
        hdr[0] = 0;  // length = 0
        hdr[1] = 0;  // capacity = 0
        return empty_list;
    }

    void* list = malloc((size_t)(16 + n * 8));
    if (!list) return NULL;
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
 *
 * C-3 fix: ftell() returns -1 for non-seekable files (e.g. /proc/*).
 * When size < 0, fall back to an incremental read so we never pass a
 * negative value to malloc().
 *
 * GRA-182 hardening:
 * - Hard cap on total bytes read (default 64 MiB, override via env
 *   `GRADIENT_FILE_READ_MAX_BYTES`).  Prevents FS-effect code from
 *   exhausting host memory when handed an arbitrary file path
 *   (oversize file, pipe, /proc/*, /dev/*, etc.).
 * - Saturating capacity growth in the non-seekable branch — `cap * 2`
 *   could wrap on a 32-bit `size_t` and produce a smaller realloc().
 * - Reject seekable files whose declared size already exceeds the cap
 *   before any allocation.
 */

/* Default cap: 64 MiB.  Tunable at runtime; never below 4 KiB to keep
 * the small-file fast path useful. */
#define GRADIENT_FILE_READ_DEFAULT_MAX (64ULL * 1024ULL * 1024ULL)
#define GRADIENT_FILE_READ_MIN_MAX     (4ULL * 1024ULL)

static size_t __gradient_file_read_max_bytes(void) {
    static size_t cached = 0;
    if (cached != 0) return cached;
    const char* env = getenv("GRADIENT_FILE_READ_MAX_BYTES");
    unsigned long long parsed = 0;
    if (env && *env) {
        char* end = NULL;
        unsigned long long v = strtoull(env, &end, 10);
        if (end != env && v >= GRADIENT_FILE_READ_MIN_MAX) {
            parsed = v;
        }
    }
    if (parsed == 0) parsed = GRADIENT_FILE_READ_DEFAULT_MAX;
    /* Clamp to SIZE_MAX - 1 so `+ 1` for the NUL terminator never overflows. */
    if (parsed > (unsigned long long)(SIZE_MAX - 1)) {
        parsed = (unsigned long long)(SIZE_MAX - 1);
    }
    cached = (size_t)parsed;
    return cached;
}

char* __gradient_file_read(const char* path) {
    FILE* f = fopen(path, "r");
    if (!f) return strdup("");

    const size_t max_bytes = __gradient_file_read_max_bytes();

    fseek(f, 0, SEEK_END);
    long size = ftell(f);
    rewind(f);

    if (size < 0) {
        /* Non-seekable file: read incrementally with a saturating, capped grow. */
        size_t cap = 4096, len = 0;
        if (cap > max_bytes) cap = max_bytes;
        char* buf = (char*)malloc(cap + 1);
        if (!buf) { fclose(f); return strdup(""); }
        size_t n;
        while ((n = fread(buf + len, 1, cap - len, f)) > 0) {
            len += n;
            if (len >= max_bytes) {
                /* Cap reached — return what we have, truncated. */
                len = max_bytes;
                break;
            }
            if (len == cap) {
                /* Saturating doubling: never overflow, never exceed max_bytes. */
                size_t new_cap;
                if (cap > max_bytes / 2) {
                    new_cap = max_bytes;
                } else {
                    new_cap = cap * 2;
                }
                if (new_cap == cap) break; /* nothing more we can grow */
                char* tmp = (char*)realloc(buf, new_cap + 1);
                if (!tmp) { free(buf); fclose(f); return strdup(""); }
                buf = tmp;
                cap = new_cap;
            }
        }
        buf[len] = '\0';
        fclose(f);
        return buf;
    }

    /* Seekable: refuse files larger than the cap before any allocation. */
    if ((unsigned long)size > (unsigned long)max_bytes) {
        fclose(f);
        return strdup("");
    }

    char* buf = (char*)malloc((size_t)size + 1);
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
int8_t __gradient_file_write(const char* path, const char* content) {
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
int8_t __gradient_file_exists(const char* path) {
    return access(path, F_OK) == 0 ? 1 : 0;
}

/*
 * __gradient_file_append(path, content) -> int64_t
 *
 * Appends `content` to the file at `path`, creating it if it does not
 * exist.  Returns 1 (true) on success, 0 (false) on failure.
 */
int8_t __gradient_file_append(const char* path, const char* content) {
    FILE* f = fopen(path, "a");
    if (!f) return 0;
    fputs(content, f);
    fclose(f);
    return 1;
}

/*
 * __gradient_file_delete(path) -> int64_t
 *
 * Deletes the file at `path`.
 * Returns 1 (true) on success, 0 (false) on failure.
 */
int8_t __gradient_file_delete(const char* path) {
    if (!path) return 0;
    return remove(path) == 0 ? 1 : 0;
}

/* ── Phase PP: Random Number Generation ────────────────────────────────────
 *
 * These functions wrap the standard C rand()/srand() functions to provide
 * random number generation for Gradient programs.
 *
 *   random()       -> Returns a random Float in range [0.0, 1.0)
 *   random_int()   -> Returns a random Int in range [min, max]
 *   random_float() -> Returns a random Float in range [0.0, 1.0)
 *   seed_random()  -> Seeds the random number generator
 */

static int __gradient_rand_initialized = 0;

/* Internal: Initialize random seed if not already done */
static void __gradient_ensure_rand_init(void) {
    if (!__gradient_rand_initialized) {
        srand((unsigned int)time(NULL));
        __gradient_rand_initialized = 1;
    }
}

/*
 * __gradient_random() -> double
 *
 * Returns a random floating-point number in the range [0.0, 1.0).
 * This is the C implementation of the Gradient random() builtin.
 */
double __gradient_random(void) {
    __gradient_ensure_rand_init();
    return (double)rand() / ((double)RAND_MAX + 1.0);
}

/*
 * __gradient_random_int(min, max) -> int64_t
 *
 * Returns a random integer in the inclusive range [min, max].
 * min and max are int64_t (signed), but we ensure the result
 * is valid even if min > max (we swap them).
 */
int64_t __gradient_random_int(int64_t min, int64_t max) {
    __gradient_ensure_rand_init();

    /* Ensure min <= max */
    if (min > max) {
        int64_t tmp = min;
        min = max;
        max = tmp;
    }

    /* Calculate the range */
    int64_t range = max - min + 1;
    if (range <= 0) {
        return min; /* Overflow protection */
    }

    /* Generate random value in range [0, range) and add to min */
    int64_t random_val = (int64_t)(rand() % (int)range);
    return min + random_val;
}

/*
 * __gradient_random_float() -> double
 *
 * Returns a random floating-point number in the range [0.0, 1.0).
 * Alias for __gradient_random() for explicit API.
 */
double __gradient_random_float(void) {
    return __gradient_random();
}

/*
 * __gradient_seed_random(seed) -> void
 *
 * Seeds the random number generator with the given int64_t seed.
 * Allows reproducible random sequences for testing.
 */
void __gradient_seed_random(int64_t seed) {
    srand((unsigned int)seed);
    __gradient_rand_initialized = 1;
}

/* ── Phase PP: Environment/Process Builtins ───────────────────────────────
 *
 * These functions provide environment variable and process operations.
 *
 *   get_env(name)      -> Get environment variable value (Option[String])
 *   set_env(name, val) -> Set environment variable (!{IO})
 *   current_dir()      -> Get current working directory (!{IO})
 *   change_dir(path)   -> Change working directory (!{IO})
 *   process_id()       -> Get current process ID (pure)
 *   sleep_seconds(s)   -> Sleep for specified seconds (!{Time})
 */

/* OptionString layout for get_env return */
typedef struct {
    int64_t tag;      /* 0 = Some, 1 = None */
    char*   payload;  /* Valid if tag == 0 */
} OptionString;

/*
 * __gradient_get_env(name: const char*) -> OptionString*
 *
 * Returns the value of an environment variable as Option[String].
 * Returns None if the variable doesn't exist.
 */
void* __gradient_get_env(const char* name) {
    OptionString* opt = (OptionString*)malloc(sizeof(OptionString));
    if (!opt) return NULL;

    if (!name) {
        opt->tag = 1; /* None */
        return opt;
    }

    const char* val = getenv(name);
    if (val) {
        opt->tag = 0; /* Some */
        opt->payload = strdup(val);
    } else {
        opt->tag = 1; /* None */
        opt->payload = NULL;
    }
    return opt;
}

/*
 * __gradient_set_env(name: const char*, value: const char*) -> void
 *
 * Sets an environment variable to the specified value.
 * If value is NULL, the variable is removed.
 */
void __gradient_set_env(const char* name, const char* value) {
    if (!name) return;

    if (value) {
        setenv(name, value, 1); /* 1 = overwrite existing */
    } else {
        unsetenv(name);
    }
}

/*
 * __gradient_current_dir() -> char*
 *
 * Returns the current working directory as a heap-allocated string.
 * Returns "<error>" if the directory cannot be determined.
 */
char* __gradient_current_dir(void) {
    char* buf = (char*)malloc(PATH_MAX);
    if (!buf) return strdup("<error>");

    if (getcwd(buf, PATH_MAX)) {
        return buf;
    } else {
        free(buf);
        return strdup("<error>");
    }
}

/*
 * __gradient_change_dir(path: const char*) -> int64_t
 *
 * Changes the current working directory.
 * Returns 0 on success, -1 on error.
 */
int64_t __gradient_change_dir(const char* path) {
    if (!path) return -1;
    return chdir(path) == 0 ? 0 : -1;
}

/*
 * M-5: __gradient_spawn(program: const char*, args: void*) -> int64_t
 *
 * Executes a program directly without invoking a shell (safer than system()).
 * Takes a program path and a Gradient List[String] of arguments.
 * Returns the process exit code (0-255), or -1 on error.
 *
 * Uses posix_spawnp() on systems that support it, or fork()+execvp() otherwise.
 * This avoids shell injection vulnerabilities since no shell is involved.
 */
int64_t __gradient_spawn(const char* program, void* args_list) {
    if (!program || !program[0]) return -1;

    /* Gradient List layout: [size: i64, capacity: i64, data...] */
    int64_t* hdr = (int64_t*)args_list;
    int64_t argc = hdr ? hdr[0] : 0;  /* length field */
    int64_t* argv_data = hdr ? (hdr + 2) : NULL;

    /* Build argv array: [program, arg1, arg2, ..., NULL] */
    /* Maximum 64 args (plus program name, plus NULL) to prevent DoS */
    #define MAX_SPAWN_ARGS 64
    char* argv[MAX_SPAWN_ARGS + 2];

    argv[0] = (char*)program;
    int i;
    for (i = 0; i < argc && i < MAX_SPAWN_ARGS; i++) {
        argv[i + 1] = (char*)(intptr_t)argv_data[i];
    }
    argv[i + 1] = NULL;

    pid_t pid;
    int status = -1;

    #ifdef _POSIX_PRIORITY_SCHEDULING
    /* Use posix_spawnp if available (POSIX.1-2001) */
    if (posix_spawnp(&pid, program, NULL, NULL, argv, environ) != 0) {
        return -1;
    }
    #else
    /* Fallback: fork() + execvp() */
    pid = fork();
    if (pid < 0) {
        return -1;
    } else if (pid == 0) {
        /* Child process */
        execvp(program, argv);
        /* If execvp returns, it failed */
        _exit(127);
    }
    /* Parent continues to wait */
    #endif

    /* Wait for child to complete */
    if (waitpid(pid, &status, 0) < 0) {
        return -1;
    }

    /* Extract exit code */
    if (WIFEXITED(status)) {
        return (int64_t)WEXITSTATUS(status);
    } else if (WIFSIGNALED(status)) {
        /* Process was killed by signal - return 128 + signal number */
        return 128 + (int64_t)WTERMSIG(status);
    }
    return -1;
}

/* ── Phase PP: Date/Time Builtins ─────────────────────────────────────────
 *
 * These functions provide date/time operations for Gradient programs.
 *
 *   now()            -> Returns Unix timestamp in seconds
 *   now_ms()         -> Returns Unix timestamp in milliseconds
 *   sleep(ms)        -> Sleep for specified milliseconds
 *   time_string()    -> Returns RFC3339 formatted timestamp string
 *   date_string()    -> Returns YYYY-MM-DD formatted date string
 *   datetime_year()  -> Extract year from timestamp
 *   datetime_month() -> Extract month (1-12) from timestamp
 *   datetime_day()   -> Extract day (1-31) from timestamp
 */

/*
 * __gradient_now() -> int64_t
 *
 * Returns the current Unix timestamp in seconds.
 */
int64_t __gradient_now(void) {
    return (int64_t)time(NULL);
}

/*
 * __gradient_now_ms() -> int64_t
 *
 * Returns the current Unix timestamp in milliseconds.
 */
int64_t __gradient_now_ms(void) {
    struct timespec ts;
    clock_gettime(CLOCK_REALTIME, &ts);
    return (int64_t)(ts.tv_sec * 1000 + ts.tv_nsec / 1000000);
}

/*
 * __gradient_sleep(ms: int64_t) -> void
 *
 * Sleep for the specified number of milliseconds.
 */
void __gradient_sleep(int64_t ms) {
    if (ms <= 0) return;
    struct timespec ts;
    ts.tv_sec = ms / 1000;
    ts.tv_nsec = (ms % 1000) * 1000000;
    nanosleep(&ts, NULL);
}

/*
 * __gradient_sleep_seconds(s: int64_t) -> void
 *
 * Sleep for the specified number of seconds.
 */
void __gradient_sleep_seconds(int64_t s) {
    if (s <= 0) return;
    struct timespec ts;
    ts.tv_sec = s;
    ts.tv_nsec = 0;
    nanosleep(&ts, NULL);
}

/*
 * __gradient_time_string() -> char*
 *
 * Returns the current time as an RFC3339 formatted string (e.g. "2026-04-03T12:34:56+00:00").
 * Caller owns the returned string (should free when done).
 */
char* __gradient_time_string(void) {
    time_t now = time(NULL);
    struct tm* tm = gmtime(&now);
    if (!tm) return strdup("");
    char* buf = (char*)malloc(26); /* Enough for RFC3339 + null terminator */
    if (!buf) return strdup("");
    strftime(buf, 26, "%Y-%m-%dT%H:%M:%S+00:00", tm);
    return buf;
}

/*
 * __gradient_date_string() -> char*
 *
 * Returns the current date as a "YYYY-MM-DD" formatted string.
 * Caller owns the returned string (should free when done).
 */
char* __gradient_date_string(void) {
    time_t now = time(NULL);
    struct tm* tm = gmtime(&now);
    if (!tm) return strdup("");
    char* buf = (char*)malloc(11); /* YYYY-MM-DD + null terminator */
    if (!buf) return strdup("");
    strftime(buf, 11, "%Y-%m-%d", tm);
    return buf;
}

/*
 * __gradient_datetime_year(ts: int64_t) -> int64_t
 *
 * Extract the year from a Unix timestamp.
 */
int64_t __gradient_datetime_year(int64_t ts) {
    time_t t = (time_t)ts;
    struct tm* tm = gmtime(&t);
    if (!tm) return 0;
    return (int64_t)(tm->tm_year + 1900);
}

/*
 * __gradient_datetime_month(ts: int64_t) -> int64_t
 *
 * Extract the month (1-12) from a Unix timestamp.
 */
int64_t __gradient_datetime_month(int64_t ts) {
    time_t t = (time_t)ts;
    struct tm* tm = gmtime(&t);
    if (!tm) return 0;
    return (int64_t)(tm->tm_mon + 1);
}

/*
 * __gradient_datetime_day(ts: int64_t) -> int64_t
 *
 * Extract the day of month (1-31) from a Unix timestamp.
 */
int64_t __gradient_datetime_day(int64_t ts) {
    time_t t = (time_t)ts;
    struct tm* tm = gmtime(&t);
    if (!tm) return 0;
    return (int64_t)tm->tm_mday;
}

/* ── Phase OO: HashMap type ────────────────────────────────────────────── */

/*
 * Option helper functions
 *
 * At runtime, Option[T] is represented as:
 *   tag: int8_t (0 = Some, 1 = None)
 *   payload: T (only present if tag == 0)
 *
 * For heap-allocated types (String, List, etc.), the payload is a pointer.
 * For immediate types (Int, Bool), the payload is stored inline.
 */

/*
 * __gradient_option_is_some(opt: void*) -> int64_t
 *
 * Returns 1 if the option is Some, 0 if None.
 */
int64_t __gradient_option_is_some(void* opt) {
    if (!opt) return 0;
    int8_t* tag = (int8_t*)opt;
    return *tag == 0 ? 1 : 0;
}

/*
 * __gradient_option_is_none(opt: void*) -> int64_t
 *
 * Returns 1 if the option is None, 0 if Some.
 */
int64_t __gradient_option_is_none(void* opt) {
    if (!opt) return 1;
    int8_t* tag = (int8_t*)opt;
    return *tag == 1 ? 1 : 0;
}

/*
 * __gradient_option_unwrap(opt: void*) -> int64_t
 *
 * Extracts the payload from a Some option. Panics on None.
 * Note: This returns the payload as int64_t for immediate types.
 * For heap-allocated types, the payload is a pointer cast to int64_t.
 */
int64_t __gradient_option_unwrap(void* opt) {
    if (!opt) {
        fprintf(stderr, "panic: called unwrap on None\n");
        exit(1);
    }
    int8_t* tag = (int8_t*)opt;
    if (*tag == 1) {
        fprintf(stderr, "panic: called unwrap on None\n");
        exit(1);
    }
    // Payload is stored after the tag byte
    int64_t* payload = (int64_t*)((int8_t*)opt + 1);
    return *payload;
}

/*
 * __gradient_option_unwrap_or(opt: void*, default_val: int64_t) -> int64_t
 *
 * Returns the payload if Some, otherwise returns the default value.
 */
int64_t __gradient_option_unwrap_or(void* opt, int64_t default_val) {
    if (!opt) return default_val;
    int8_t* tag = (int8_t*)opt;
    if (*tag == 1) return default_val;
    // Payload is stored after the tag byte
    int64_t* payload = (int64_t*)((int8_t*)opt + 1);
    return *payload;
}

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
 *       int      ref_count;   // reference counter for COW semantics
 *       int      is_cow_copy; // flag indicating this is a COW copy
 *   } GradientMap;
 *
 * The struct is 40 bytes on 64-bit platforms.
 * Codegen accesses these fields at byte offsets:
 *   offset  0: size     (i64)
 *   offset  8: capacity (i64)
 *   offset 16: keys ptr (i64 — pointer)
 *   offset 24: values ptr (i64 — pointer)
 *   offset 32: ref_count (int)
 *   offset 36: is_cow_copy (int)
 *
 * We use a simple sorted-key linear-search strategy (O(n)) which is correct
 * for all map sizes encountered in practice.  A hash table upgrade is future
 * work.
 *
 * Copy-on-Write (COW) Semantics:
 *   - map_new() creates a map with ref_count = 1
 *   - map_copy() does a shallow retain (ref_count++) instead of deep copy
 *   - map_set/remove use map_make_mutable() which clones only if ref_count > 1
 *   - map_release() decrements ref_count and frees only when it reaches 0
 * This optimization prevents unnecessary copying when maps are shared.
 */

#define GRADIENT_MAP_INIT_CAP 8

typedef struct {
    int64_t  size;
    int64_t  capacity;
    char**   keys;
    int64_t* values;
    int      ref_count;   /* Reference counter for COW semantics */
    int      is_cow_copy; /* Flag indicating this is a COW copy */
} GradientMap;

static GradientMap* map_alloc(int64_t cap) {
    GradientMap* m = (GradientMap*)malloc(sizeof(GradientMap));
    m->size        = 0;
    m->capacity    = cap;
    m->keys        = (char**)calloc((size_t)cap, sizeof(char*));
    m->values      = (int64_t*)calloc((size_t)cap, sizeof(int64_t));
    m->ref_count   = 1;
    m->is_cow_copy = 0;
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
 * map_retain(map)
 *
 * Increment the reference count of a map.
 * Returns the map pointer for chaining.
 */
static GradientMap* map_retain(GradientMap* m) {
    if (!m) return NULL;
    m->ref_count++;
    m->is_cow_copy = 1;  /* Mark as COW copy once retained */
    return m;
}

/*
 * map_release(map)
 *
 * Decrement the reference count and free the map if it reaches 0.
 * Replaces map_destroy with reference counting semantics.
 *
 * Note: For String value maps, caller must ensure values are freed.
 * This version frees keys only (safe for all map types).
 */
void map_release(void* map) {
    GradientMap* m = (GradientMap*)map;
    if (!m) return;
    m->ref_count--;
    if (m->ref_count <= 0) {
        for (int64_t i = 0; i < m->size; i++) {
            if (m->keys[i]) free(m->keys[i]);
        }
        free(m->keys);
        free(m->values);
        free(m);
    }
}

/*
 * map_destroy(map)
 *
 * DEPRECATED: Use map_release() instead.
 * Kept for backward compatibility during transition.
 */
void map_destroy(void* map) {
    map_release(map);
}

/*
 * map_make_mutable(map)
 *
 * Copy-on-Write: Returns a mutable copy of the map if ref_count > 1,
 * otherwise returns the map itself (now exclusive).
 * Callers must use the returned pointer.
 */
static GradientMap* map_make_mutable(GradientMap* m) {
    if (!m || m->ref_count <= 1) return m;  /* Already exclusive */

    /* Need to clone for COW - deep copy since we're becoming the owner */
    GradientMap* clone = map_alloc(m->capacity);
    clone->size = m->size;

    /* Deep copy keys */
    for (int64_t i = 0; i < m->size; i++) {
        if (m->keys[i]) {
            clone->keys[i] = strdup(m->keys[i]);
        }
        clone->values[i] = m->values[i];
    }

    /* Retain others, release our reference to original */
    m->ref_count--;
    m->is_cow_copy = 1;

    /* Return the new exclusive copy with ref_count = 1 */
    return clone;
}

/*
 * map_release_str_values(map)
 *
 * Free a GradientMap AND all string values. Use this for Map[String, String].
 * Decrements ref_count, only frees when ref_count reaches 0.
 */
void map_release_str_values(void* map) {
    GradientMap* m = (GradientMap*)map;
    if (!m) return;
    m->ref_count--;
    if (m->ref_count <= 0) {
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
}

/*
 * map_destroy_str_values(map)
 *
 * DEPRECATED: Use map_release_str_values() instead.
 * Kept for backward compatibility during transition.
 */
void map_destroy_str_values(void* map) {
    map_release_str_values(map);
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
    m->keys   = (char**)safe_realloc(m->keys,   (size_t)new_cap * sizeof(char*));
    m->values = (int64_t*)safe_realloc(m->values, (size_t)new_cap * sizeof(int64_t));
    /* Zero out new slots. */
    for (int64_t i = m->capacity; i < new_cap; i++) {
        m->keys[i]   = NULL;
        m->values[i] = 0;
    }
    m->capacity = new_cap;
}

/* Internal: copy a map (retain — shallow copy that increments ref_count).
 * This is the key optimization: instead of deep copying, we retain the
 * original and use copy-on-write (COW) for modifications.
 */
static GradientMap* map_copy(GradientMap* src) {
    /* Instead of deep copy, retain the original map */
    return map_retain(src);
}

/*
 * map_deep_copy(map)
 *
 * Create a true deep copy of a map. Used internally when COW requires
 * actual duplication. Returns a new map with ref_count = 1.
 */
static GradientMap* map_deep_copy(GradientMap* src) {
    if (!src) return NULL;
    GradientMap* dst = map_alloc(src->capacity);
    dst->size     = src->size;
    for (int64_t i = 0; i < src->size; i++) {
        dst->keys[i]   = src->keys[i] ? strdup(src->keys[i]) : NULL;
        dst->values[i] = src->values[i];
    }
    return dst;
}

/*
 * __gradient_map_set_str(map, key, value) -> GradientMap*
 *
 * Insert or update a Map[String, String] entry.  Returns the (possibly
 * reallocated) map pointer.
 * Uses Copy-on-Write (COW): if ref_count > 1, creates a mutable copy first.
 */
void* __gradient_map_set_str(void* map, const char* key, const char* value) {
    GradientMap* m = map_make_mutable((GradientMap*)map);

    int64_t idx = map_find(m, key);
    if (idx >= 0) {
        /* Update existing entry.
         * String maps own their duplicated string values; replacing an
         * existing entry must release the old value before storing the new
         * duplicate.
         */
        free(m->keys[idx]);
        if (m->values[idx]) {
            free((char*)(intptr_t)m->values[idx]);
        }
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
 * Uses Copy-on-Write (COW): if ref_count > 1, creates a mutable copy first.
 */
void* __gradient_map_set_int(void* map, const char* key, int64_t value) {
    GradientMap* m = map_make_mutable((GradientMap*)map);

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
 * Uses Copy-on-Write (COW): if ref_count > 1, creates a mutable copy first.
 */
void* __gradient_map_remove(void* map, const char* key) {
    GradientMap* m = map_make_mutable((GradientMap*)map);

    int64_t idx = map_find(m, key);
    if (idx < 0) return (void*)m;  /* key not present, return as-is */

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

/* ── Phase PP: Set Operations ────────────────────────────────────────────── */

#define GRADIENT_SET_INIT_CAP 16

/*
 * GradientSet: hash-based set for i64 values.
 *
 * Layout:
 *   size:     number of elements currently stored
 *   capacity: size of the buckets array
 *   buckets:  array of int64_t values, where 0 = empty slot
 *
 * Uses simple linear probing for collision resolution.
 * Value 0 cannot be stored (reserved for empty marker).
 */
typedef struct {
    int64_t  size;
    int64_t  capacity;
    int64_t* buckets;
    int64_t  ref_count;
    int64_t  is_cow_copy;
} GradientSet;

/*
 * Reference counting functions for GradientSet
 */

/* Increment reference count */
static void set_retain(GradientSet* s) {
    if (s) {
        s->ref_count++;
    }
}

/* Decrement reference count and free if zero */
static void set_release(GradientSet* s) {
    if (!s) return;
    s->ref_count--;
    if (s->ref_count <= 0) {
        free(s->buckets);
        free(s);
    }
}

/* Make a set mutable: clone if ref_count > 1 (Copy-on-Write) */
static GradientSet* set_make_mutable(GradientSet* s) {
    if (!s) return NULL;
    if (s->ref_count <= 1) {
        /* Exclusive ownership, can modify directly */
        s->is_cow_copy = 0;
        return s;
    }
    /* Shared copy, need to clone */
    s->ref_count--;
    GradientSet* dst = (GradientSet*)malloc(sizeof(GradientSet));
    dst->size = s->size;
    dst->capacity = s->capacity;
    dst->buckets = (int64_t*)malloc((size_t)s->capacity * sizeof(int64_t));
    memcpy(dst->buckets, s->buckets, (size_t)s->capacity * sizeof(int64_t));
    dst->ref_count = 1;
    dst->is_cow_copy = 0;
    return dst;
}

/* Hash function for int64_t (FNV-1a style mixing). */
static uint64_t set_hash_int64(int64_t value) {
    /* FNV-1a 64-bit hash */
    uint64_t hash = 0xcbf29ce484222325ULL;
    uint64_t v = (uint64_t)value;
    for (int i = 0; i < 8; i++) {
        hash ^= (v >> (i * 8)) & 0xFF;
        hash *= 0x100000001b3ULL;
    }
    return hash;
}

/* Find the index for a value, returning -1 if not found. */
static int64_t set_find_index(GradientSet* s, int64_t value) {
    if (s->size == 0) return -1;
    uint64_t hash = set_hash_int64(value);
    int64_t idx = (int64_t)(hash % (uint64_t)s->capacity);
    int64_t start_idx = idx;

    while (s->buckets[idx] != 0) {
        if (s->buckets[idx] == value) {
            return idx;
        }
        idx = (idx + 1) % s->capacity;
        if (idx == start_idx) break;  /* Full circle */
    }
    return -1;
}

/* Find insert position (either existing slot or first empty). */
static int64_t set_find_insert_pos(GradientSet* s, int64_t value) {
    uint64_t hash = set_hash_int64(value);
    int64_t idx = (int64_t)(hash % (uint64_t)s->capacity);
    int64_t start_idx = idx;

    while (s->buckets[idx] != 0) {
        if (s->buckets[idx] == value) {
            return idx;  /* Already exists */
        }
        idx = (idx + 1) % s->capacity;
        if (idx == start_idx) return -1;  /* Table full */
    }
    return idx;  /* Empty slot found */
}

static void set_grow(GradientSet* s);

/* Allocate a new empty set. */
static GradientSet* set_alloc(int64_t cap) {
    GradientSet* s = (GradientSet*)malloc(sizeof(GradientSet));
    s->size = 0;
    s->capacity = cap;
    s->buckets = (int64_t*)calloc((size_t)cap, sizeof(int64_t));
    s->ref_count = 1;
    s->is_cow_copy = 0;
    return s;
}

/*
 * Copy a set for modification (COW semantics).
 * This retains the original set and returns a new reference.
 * When the caller needs to modify, they should use set_make_mutable.
 */
static GradientSet* set_copy(GradientSet* src) {
    set_retain(src);
    src->is_cow_copy = 1;
    return src;
}

/* Grow the set when load factor exceeds 0.75. */
static void set_grow(GradientSet* s) {
    int64_t old_cap = s->capacity;
    int64_t* old_buckets = s->buckets;

    int64_t new_cap = old_cap * 2;
    s->buckets = (int64_t*)calloc((size_t)new_cap, sizeof(int64_t));
    s->capacity = new_cap;
    s->size = 0;

    /* Rehash all elements. */
    for (int64_t i = 0; i < old_cap; i++) {
        if (old_buckets[i] != 0) {
            int64_t pos = set_find_insert_pos(s, old_buckets[i]);
            if (pos >= 0) {
                s->buckets[pos] = old_buckets[i];
                s->size++;
            }
        }
    }
    free(old_buckets);
}

/*
 * __gradient_set_new() -> GradientSet*
 *
 * Create and return an empty set.
 */
void* __gradient_set_new(void) {
    return (void*)set_alloc(GRADIENT_SET_INIT_CAP);
}

/*
 * __gradient_set_add(set, elem) -> GradientSet*
 *
 * Add an element to the set. Returns a new set (persistent copy with COW).
 * Note: elem = 0 is not allowed (reserved for empty marker).
 */
void* __gradient_set_add(void* set, int64_t elem) {
    GradientSet* s = set_make_mutable((GradientSet*)set);

    if (elem == 0) {
        /* Cannot store 0, return unchanged. */
        return (void*)s;
    }

    /* Check if exists. */
    if (set_find_index(s, elem) >= 0) {
        /* Already exists, return unchanged. */
        return (void*)s;
    }

    /* Grow if load factor > 0.75. */
    if (s->size * 4 >= s->capacity * 3) {
        set_grow(s);
    }

    int64_t pos = set_find_insert_pos(s, elem);
    if (pos >= 0 && s->buckets[pos] == 0) {
        s->buckets[pos] = elem;
        s->size++;
    }
    return (void*)s;
}

/*
 * __gradient_set_remove(set, elem) -> GradientSet*
 *
 * Remove an element from the set. Returns a new set (persistent copy with COW).
 */
void* __gradient_set_remove(void* set, int64_t elem) {
    GradientSet* s = set_make_mutable((GradientSet*)set);

    if (elem == 0) {
        return (void*)s;
    }

    int64_t idx = set_find_index(s, elem);
    if (idx >= 0) {
        s->buckets[idx] = 0;
        s->size--;
        /* Note: We're not rehashing here. This can lead to issues with
         * linear probing if many items are deleted. For simplicity,
         * we use tombstones in a production implementation. */
    }
    return (void*)s;
}

/*
 * __gradient_set_contains(set, elem) -> int64_t
 *
 * Returns 1 if element is in the set, 0 otherwise.
 */
int64_t __gradient_set_contains(void* set, int64_t elem) {
    GradientSet* s = (GradientSet*)set;
    if (elem == 0) return 0;
    return set_find_index(s, elem) >= 0 ? 1 : 0;
}

/*
 * __gradient_set_size(set) -> int64_t
 *
 * Returns the number of elements in the set.
 */
int64_t __gradient_set_size(void* set) {
    GradientSet* s = (GradientSet*)set;
    return s->size;
}

/*
 * __gradient_set_union(a, b) -> GradientSet*
 *
 * Returns a new set containing all elements from both sets.
 */
void* __gradient_set_union(void* a, void* b) {
    GradientSet* set_a = (GradientSet*)a;
    GradientSet* set_b = (GradientSet*)b;

    /* Start with copy of set_a and make mutable for modifications. */
    GradientSet* result = set_make_mutable(set_copy(set_a));

    /* Add all elements from set_b. */
    for (int64_t i = 0; i < set_b->capacity; i++) {
        if (set_b->buckets[i] != 0) {
            /* Add element (handles duplicates and resizing). */
            if (set_find_index(result, set_b->buckets[i]) < 0) {
                /* Need to add */
                if (result->size * 4 >= result->capacity * 3) {
                    set_grow(result);
                }
                int64_t pos = set_find_insert_pos(result, set_b->buckets[i]);
                if (pos >= 0 && result->buckets[pos] == 0) {
                    result->buckets[pos] = set_b->buckets[i];
                    result->size++;
                }
            }
        }
    }
    return (void*)result;
}

/*
 * __gradient_set_intersection(a, b) -> GradientSet*
 *
 * Returns a new set containing only elements present in both sets.
 */
void* __gradient_set_intersection(void* a, void* b) {
    GradientSet* set_a = (GradientSet*)a;

    /* Start with empty set. */
    GradientSet* result = set_alloc(GRADIENT_SET_INIT_CAP);

    /* Add elements from a that are also in b. */
    for (int64_t i = 0; i < set_a->capacity; i++) {
        if (set_a->buckets[i] != 0) {
            if (__gradient_set_contains(b, set_a->buckets[i])) {
                /* Add to result */
                if (result->size * 4 >= result->capacity * 3) {
                    set_grow(result);
                }
                int64_t pos = set_find_insert_pos(result, set_a->buckets[i]);
                if (pos >= 0 && result->buckets[pos] == 0) {
                    result->buckets[pos] = set_a->buckets[i];
                    result->size++;
                }
            }
        }
    }
    return (void*)result;
}

/*
 * __gradient_set_to_list(set) -> List[Int]
 *
 * Returns a Gradient list containing all elements of the set.
 */
void* __gradient_set_to_list(void* set) {
    GradientSet* s = (GradientSet*)set;
    int64_t n = s->size;

    /* Gradient list: 16-byte header + n * 8 bytes data. */
    void* list = malloc((size_t)(16 + n * 8));
    int64_t* hdr = (int64_t*)list;
    hdr[0] = n;   /* length   */
    hdr[1] = n;   /* capacity */
    int64_t* data = hdr + 2;

    int64_t idx = 0;
    for (int64_t i = 0; i < s->capacity && idx < n; i++) {
        if (s->buckets[i] != 0) {
            data[idx++] = s->buckets[i];
        }
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
    buf->data = (char*)safe_realloc(buf->data, buf->size + total + 1);
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

    /* C-5: restrict protocols, enforce TLS verification, cap redirects. */
    curl_easy_setopt(curl, CURLOPT_PROTOCOLS_STR, "https");
    curl_easy_setopt(curl, CURLOPT_REDIR_PROTOCOLS_STR, "https");
    curl_easy_setopt(curl, CURLOPT_MAXREDIRS, 5L);
    curl_easy_setopt(curl, CURLOPT_SSL_VERIFYPEER, 1L);
    curl_easy_setopt(curl, CURLOPT_SSL_VERIFYHOST, 2L);
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

    /* C-5: restrict protocols, enforce TLS verification, cap redirects. */
    curl_easy_setopt(curl, CURLOPT_PROTOCOLS_STR, "https");
    curl_easy_setopt(curl, CURLOPT_REDIR_PROTOCOLS_STR, "https");
    curl_easy_setopt(curl, CURLOPT_MAXREDIRS, 5L);
    curl_easy_setopt(curl, CURLOPT_SSL_VERIFYPEER, 1L);
    curl_easy_setopt(curl, CURLOPT_SSL_VERIFYHOST, 2L);
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

/* H-4: maximum nesting depth for JSON arrays and objects. */
#define MAX_JSON_DEPTH 128

typedef struct {
    const char* input;
    size_t pos;
    int depth;
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
            /* H-3: safe_realloc aborts on OOM rather than silently leaking. */
            buf = (char*)safe_realloc(buf, cap);
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
    /* H-4: depth-bomb guard. */
    if (p->depth >= MAX_JSON_DEPTH) {
        json_set_error(p, "JSON nesting depth limit (%d) exceeded", MAX_JSON_DEPTH);
        return NULL;
    }
    p->depth++;
    p->pos++;
    json_skip_ws(p);

    int64_t cap = 8;
    int64_t len = 0;
    int64_t* items = (int64_t*)malloc((size_t)(cap * 8));
    if (!items) {
        p->depth--;
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
                p->depth--;
                return NULL;
            }

            if (len >= cap) {
                cap *= 2;
                /* H-3: safe_realloc aborts on OOM rather than silently leaking. */
                items = (int64_t*)safe_realloc(items, (size_t)(cap * 8));
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
            p->depth--;
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
        p->depth--;
        json_set_error(p, "out of memory while finalizing array");
        return NULL;
    }
    list[0] = len;
    list[1] = len;
    memcpy(list + 2, items, (size_t)(len * 8));
    free(items);
    p->depth--;
    return json_array((void*)list);
}

static void* json_parse_object(JsonParser* p) {
    /* H-4: depth-bomb guard. */
    if (p->depth >= MAX_JSON_DEPTH) {
        json_set_error(p, "JSON nesting depth limit (%d) exceeded", MAX_JSON_DEPTH);
        return NULL;
    }
    p->depth++;
    p->pos++;
    json_skip_ws(p);

    GradientMap* map = (GradientMap*)__gradient_map_new();
    if (!map) {
        p->depth--;
        json_set_error(p, "out of memory while parsing object");
        return NULL;
    }

    if (p->input[p->pos] != '}') {
        while (1) {
            json_skip_ws(p);
            char* key = json_parse_string_raw(p);
            if (!key) {
                map_destroy(map);
                p->depth--;
                return NULL;
            }

            json_skip_ws(p);
            if (p->input[p->pos] != ':') {
                free(key);
                map_destroy(map);
                p->depth--;
                json_set_error(p, "expected ':' at pos %zu", p->pos);
                return NULL;
            }
            p->pos++;

            json_skip_ws(p);
            void* val = json_parse_value(p);
            if (!val) {
                free(key);
                map_destroy(map);
                p->depth--;
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
            p->depth--;
            json_set_error(p, "expected ',' or '}' at pos %zu", p->pos);
            return NULL;
        }
    }

    p->pos++;
    p->depth--;
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
    JsonParser parser = { .input = input ? input : "", .pos = 0, .depth = 0, .error = {0} };
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
        *buf = (char*)safe_realloc(*buf, *cap);
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

int64_t __gradient_json_has(void* val, const char* key) {
    if (!val || !key) return 0;
    if (((int64_t*)val)[0] != JSON_OBJECT) return 0;
    GradientMap* m = (GradientMap*)(intptr_t)((int64_t*)val)[1];
    return map_find(m, key) >= 0 ? 1 : 0;
}

void* __gradient_json_keys(void* val) {
    if (!val || ((int64_t*)val)[0] != JSON_OBJECT) {
        int64_t* empty = (int64_t*)malloc(16);
        empty[0] = 0;
        empty[1] = 0;
        return empty;
    }
    GradientMap* m = (GradientMap*)(intptr_t)((int64_t*)val)[1];
    return __gradient_map_keys((void*)m);
}

int64_t __gradient_json_len(void* val) {
    if (!val) return 0;
    int64_t tag = ((int64_t*)val)[0];
    if (tag == JSON_ARRAY) {
        int64_t* list = (int64_t*)(intptr_t)((int64_t*)val)[1];
        return list[0];
    }
    if (tag == JSON_OBJECT) {
        GradientMap* m = (GradientMap*)(intptr_t)((int64_t*)val)[1];
        return m->size;
    }
    return 0;
}

void* __gradient_json_array_get(void* val, int64_t index) {
    if (!val || ((int64_t*)val)[0] != JSON_ARRAY) return NULL;
    int64_t* list = (int64_t*)(intptr_t)((int64_t*)val)[1];
    int64_t len = list[0];
    if (index < 0 || index >= len) return NULL;
    int64_t* data = list + 2;
    return (void*)(intptr_t)data[index];
}

/* ── Phase PP: JSON typed extractors ─────────────────────────────────────── */

/*
 * Typed primitive extractors for JsonValue.
 * Each returns an Option[T] via pointer:
 *   Some(value) -> malloc'd {tag=0, payload=value}
 *   None        -> malloc'd {tag=1}
 */

typedef struct { int64_t tag; int64_t payload; } OptionInt64;
typedef struct { int64_t tag; double payload; } OptionFloat64;
typedef struct { int64_t tag; int8_t payload; } OptionBool;

/* json_as_string(value) -> Option[String] */
void* __gradient_json_as_string(void* val) {
    OptionString* opt = (OptionString*)malloc(sizeof(OptionString));
    if (!val || ((int64_t*)val)[0] != JSON_STRING) {
        opt->tag = 1; /* None */
        return opt;
    }
    opt->tag = 0; /* Some */
    opt->payload = strdup((char*)(intptr_t)((int64_t*)val)[1]);
    return opt;
}

/* json_as_int(value) -> Option[Int] */
void* __gradient_json_as_int(void* val) {
    OptionInt64* opt = (OptionInt64*)malloc(sizeof(OptionInt64));
    if (!val || ((int64_t*)val)[0] != JSON_INT) {
        opt->tag = 1;
        return opt;
    }
    opt->tag = 0;
    opt->payload = ((int64_t*)val)[1];
    return opt;
}

/* json_as_float(value) -> Option[Float] */
void* __gradient_json_as_float(void* val) {
    OptionFloat64* opt = (OptionFloat64*)malloc(sizeof(OptionFloat64));
    if (!val) {
        opt->tag = 1;
        return opt;
    }
    int64_t tag = ((int64_t*)val)[0];
    if (tag == JSON_FLOAT) {
        opt->tag = 0;
        memcpy(&opt->payload, &((int64_t*)val)[1], sizeof(double));
    } else if (tag == JSON_INT) {
        opt->tag = 0;
        opt->payload = (double)((int64_t*)val)[1];
    } else {
        opt->tag = 1;
    }
    return opt;
}

/* json_as_bool(value) -> Option[Bool] */
void* __gradient_json_as_bool(void* val) {
    OptionBool* opt = (OptionBool*)malloc(sizeof(OptionBool));
    if (!val || ((int64_t*)val)[0] != JSON_BOOL) {
        opt->tag = 1;
        return opt;
    }
    opt->tag = 0;
    opt->payload = ((int64_t*)val)[1] ? 1 : 0;
    return opt;
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

/* ── Phase PP: String Utilities ────────────────────────────────────────── */

/* string_join(strings: List[String], separator: String) -> String */
char* __gradient_string_join(void* strings, const char* separator) {
    if (!strings) return strdup("");

    int64_t* hdr = (int64_t*)strings;
    int64_t n = hdr[0];  /* length */
    char** data = (char**)(hdr + 2);

    if (n == 0) return strdup("");

    /* Calculate total length needed */
    size_t sep_len = strlen(separator);
    size_t total_len = 0;
    for (int64_t i = 0; i < n; i++) {
        if (data[i]) {
            total_len += strlen(data[i]);
        }
        if (i < n - 1) total_len += sep_len;
    }

    /* Allocate and build result */
    char* result = (char*)malloc(total_len + 1);
    if (!result) return strdup("");

    result[0] = '\0';
    for (int64_t i = 0; i < n; i++) {
        if (data[i]) {
            strcat(result, data[i]);
        }
        if (i < n - 1 && separator) {
            strcat(result, separator);
        }
    }

    return result;
}

/* string_repeat(s: String, n: Int) -> String */
char* __gradient_string_repeat(const char* s, int64_t n) {
    if (!s || n <= 0) return strdup("");

    size_t len = strlen(s);
    size_t total_len = len * (size_t)n;

    char* result = (char*)malloc(total_len + 1);
    if (!result) return strdup("");

    result[0] = '\0';
    for (int64_t i = 0; i < n; i++) {
        strcat(result, s);
    }

    return result;
}

/* string_pad_left(s: String, n: Int, pad: String) -> String */
char* __gradient_string_pad_left(const char* s, int64_t n, const char* pad) {
    if (!s) s = "";
    if (!pad || n <= 0) return strdup(s);

    size_t s_len = strlen(s);
    if ((int64_t)s_len >= n) return strdup(s);

    size_t pad_len = strlen(pad);
    size_t pad_count = (n - s_len) / pad_len;
    size_t extra = (n - s_len) % pad_len;

    char* result = (char*)malloc(n + 1);
    if (!result) return strdup(s);

    result[0] = '\0';

    /* Add full pad strings */
    for (size_t i = 0; i < pad_count; i++) {
        strcat(result, pad);
    }
    /* Add partial pad if needed */
    if (extra > 0) {
        strncat(result, pad, extra);
    }
    /* Add original string */
    strcat(result, s);

    return result;
}

/* string_pad_right(s: String, n: Int, pad: String) -> String */
char* __gradient_string_pad_right(const char* s, int64_t n, const char* pad) {
    if (!s) s = "";
    if (!pad || n <= 0) return strdup(s);

    size_t s_len = strlen(s);
    if ((int64_t)s_len >= n) return strdup(s);

    size_t pad_len = strlen(pad);
    size_t pad_count = (n - s_len) / pad_len;
    size_t extra = (n - s_len) % pad_len;

    char* result = (char*)malloc(n + 1);
    if (!result) return strdup(s);

    strcpy(result, s);

    /* Add full pad strings */
    for (size_t i = 0; i < pad_count; i++) {
        strcat(result, pad);
    }
    /* Add partial pad if needed */
    if (extra > 0) {
        strncat(result, pad, extra);
    }

    return result;
}

/* string_strip(s: String) -> String (same as trim) */
char* __gradient_string_strip(const char* s) {
    if (!s) return strdup("");

    /* Find start (skip leading whitespace) */
    const char* start = s;
    while (*start && isspace((unsigned char)*start)) {
        start++;
    }

    /* Find end (skip trailing whitespace) */
    const char* end = s + strlen(s) - 1;
    while (end > start && isspace((unsigned char)*end)) {
        end--;
    }

    size_t len = end - start + 1;
    char* result = (char*)malloc(len + 1);
    if (!result) return strdup("");

    memcpy(result, start, len);
    result[len] = '\0';

    return result;
}

/* string_strip_prefix(s: String, prefix: String) -> Option[String] */
void* __gradient_string_strip_prefix(const char* s, const char* prefix) {
    OptionString* opt = (OptionString*)malloc(sizeof(OptionString));
    if (!opt) return NULL;

    if (!s || !prefix || strncmp(s, prefix, strlen(prefix)) != 0) {
        opt->tag = 1; /* None */
        return opt;
    }

    opt->tag = 0; /* Some */
    opt->payload = strdup(s + strlen(prefix));
    return opt;
}

/* string_strip_suffix(s: String, suffix: String) -> Option[String] */
void* __gradient_string_strip_suffix(const char* s, const char* suffix) {
    OptionString* opt = (OptionString*)malloc(sizeof(OptionString));
    if (!opt) return NULL;

    if (!s || !suffix) {
        opt->tag = 1; /* None */
        return opt;
    }

    size_t s_len = strlen(s);
    size_t suffix_len = strlen(suffix);

    if (suffix_len > s_len || strcmp(s + s_len - suffix_len, suffix) != 0) {
        opt->tag = 1; /* None */
        return opt;
    }

    opt->tag = 0; /* Some */
    opt->payload = (char*)malloc(s_len - suffix_len + 1);
    memcpy(opt->payload, s, s_len - suffix_len);
    opt->payload[s_len - suffix_len] = '\0';
    return opt;
}

/* string_to_int(s: String) -> Option[Int] */
void* __gradient_string_to_int(const char* s) {
    OptionInt64* opt = (OptionInt64*)malloc(sizeof(OptionInt64));
    if (!opt) return NULL;

    if (!s || *s == '\0') {
        opt->tag = 1; /* None */
        return opt;
    }

    char* endptr;
    errno = 0;
    long long val = strtoll(s, &endptr, 10);

    if (endptr == s || *endptr != '\0' || errno == ERANGE) {
        opt->tag = 1; /* None */
        return opt;
    }

    opt->tag = 0; /* Some */
    opt->payload = (int64_t)val;
    return opt;
}

/* string_to_float(s: String) -> Option[Float] */
void* __gradient_string_to_float(const char* s) {
    OptionFloat64* opt = (OptionFloat64*)malloc(sizeof(OptionFloat64));
    if (!opt) return NULL;

    if (!s || *s == '\0') {
        opt->tag = 1; /* None */
        return opt;
    }

    char* endptr;
    errno = 0;
    double val = strtod(s, &endptr);

    if (endptr == s || *endptr != '\0' || errno == ERANGE) {
        opt->tag = 1; /* None */
        return opt;
    }

    opt->tag = 0; /* Some */
    opt->payload = val;
    return opt;
}

/* ============================================================================
 * Queue Implementation (Phase PP)
 * ============================================================================
 *
 * Queue is implemented as a linked list with head and tail pointers for O(1)
 * enqueue and dequeue operations.
 */

/* Queue node structure - linked list node */
typedef struct GradientQueueNode {
    int64_t value;
    struct GradientQueueNode* next;
} GradientQueueNode;

/* Queue structure with head and tail pointers */
typedef struct {
    GradientQueueNode* head;
    GradientQueueNode* tail;
    int64_t size;
    int64_t ref_count;
} GradientQueue;

/* Option for (value, queue) tuple used by dequeue */
typedef struct {
    int64_t tag;              /* 0 = Some, 1 = None */
    struct {
        int64_t value;
        GradientQueue* queue;
    } payload;
} OptionInt64Queue;

/* Option for peek operation */
typedef struct {
    int64_t tag;              /* 0 = Some, 1 = None */
    int64_t payload;
} OptionInt64Peek;

/*
 * Reference counting functions for GradientQueue
 */

/* Increment reference count */
static void queue_retain(GradientQueue* q) {
    if (q) {
        q->ref_count++;
    }
}

/* Decrement reference count and free if zero */
static void queue_release(GradientQueue* q) {
    if (!q) return;
    q->ref_count--;
    if (q->ref_count <= 0) {
        /* Free all nodes */
        GradientQueueNode* node = q->head;
        while (node) {
            GradientQueueNode* next = node->next;
            free(node);
            node = next;
        }
        free(q);
    }
}

/* Make a queue mutable: clone if ref_count > 1 (Copy-on-Write) */
static GradientQueue* queue_make_mutable(GradientQueue* q) {
    if (!q) return NULL;
    if (q->ref_count <= 1) {
        /* Exclusive ownership, can modify directly */
        return q;
    }
    /* Shared copy, need to deep copy */
    q->ref_count--;
    GradientQueue* new_q = (GradientQueue*)malloc(sizeof(GradientQueue));
    if (!new_q) return NULL;
    new_q->size = q->size;
    new_q->ref_count = 1;

    /* Deep copy nodes */
    GradientQueueNode* new_head = NULL;
    GradientQueueNode* new_tail = NULL;
    GradientQueueNode* cur = q->head;
    while (cur) {
        GradientQueueNode* new_node = (GradientQueueNode*)malloc(sizeof(GradientQueueNode));
        if (!new_node) {
            /* Cleanup on error */
            while (new_head) {
                GradientQueueNode* next = new_head->next;
                free(new_head);
                new_head = next;
            }
            free(new_q);
            return NULL;
        }
        new_node->value = cur->value;
        new_node->next = NULL;

        if (!new_head) {
            new_head = new_node;
            new_tail = new_node;
        } else {
            new_tail->next = new_node;
            new_tail = new_node;
        }
        cur = cur->next;
    }
    new_q->head = new_head;
    new_q->tail = new_tail;
    return new_q;
}

/* queue_new() -> Queue[T] */
void* __gradient_queue_new(void) {
    GradientQueue* q = (GradientQueue*)malloc(sizeof(GradientQueue));
    if (!q) return NULL;
    q->head = NULL;
    q->tail = NULL;
    q->size = 0;
    q->ref_count = 1;
    return q;
}

/* queue_enqueue(q: Queue[T], item: T) -> Queue[T] */
/* Note: Uses COW semantics - clones if ref_count > 1 */
void* __gradient_queue_enqueue(GradientQueue* q, int64_t item) {
    if (!q) return NULL;

    /* Make mutable (copy if shared) */
    GradientQueue* mutable_q = queue_make_mutable(q);
    if (!mutable_q) return NULL;

    /* Create new node */
    GradientQueueNode* node = (GradientQueueNode*)malloc(sizeof(GradientQueueNode));
    if (!node) {
        return NULL;
    }
    node->value = item;
    node->next = NULL;

    /* Add new node to tail */
    if (mutable_q->tail) {
        mutable_q->tail->next = node;
        mutable_q->tail = node;
    } else {
        /* Queue was empty */
        mutable_q->head = node;
        mutable_q->tail = node;
    }
    mutable_q->size++;

    return mutable_q;
}

/* queue_dequeue(q: Queue[T]) -> Option[(T, Queue[T])] */
void* __gradient_queue_dequeue(GradientQueue* q) {
    OptionInt64Queue* opt = (OptionInt64Queue*)malloc(sizeof(OptionInt64Queue));
    if (!opt) return NULL;

    if (!q || !q->head) {
        opt->tag = 1; /* None */
        return opt;
    }

    /* Get the head value */
    int64_t value = q->head->value;

    /* Make mutable (copy if shared) */
    GradientQueue* mutable_q = queue_make_mutable(q);
    if (!mutable_q) {
        opt->tag = 1; /* None */
        return opt;
    }

    /* Remove head node */
    GradientQueueNode* old_head = mutable_q->head;
    mutable_q->head = old_head->next;
    if (!mutable_q->head) {
        mutable_q->tail = NULL;
    }
    mutable_q->size--;

    /* Free the removed node */
    free(old_head);

    /* Return Some((value, mutable_q)) */
    opt->tag = 0; /* Some */
    opt->payload.value = value;
    opt->payload.queue = mutable_q;
    return opt;
}

/* queue_peek(q: Queue[T]) -> Option[T] */
void* __gradient_queue_peek(GradientQueue* q) {
    OptionInt64Peek* opt = (OptionInt64Peek*)malloc(sizeof(OptionInt64Peek));
    if (!opt) return NULL;

    if (!q || !q->head) {
        opt->tag = 1; /* None */
        return opt;
    }

    opt->tag = 0; /* Some */
    opt->payload = q->head->value;
    return opt;
}

/* queue_size(q: Queue[T]) -> Int */
int64_t __gradient_queue_size(GradientQueue* q) {
    if (!q) return 0;
    return q->size;
}

/* ============================================================================
 * Phase PP: Stack Builtins
 * LIFO (Last-In-First-Out) stack with O(1) push and pop operations.
 * ============================================================================ */

/* Stack node structure (linked list) */
typedef struct GradientStackNode {
    int64_t value;
    struct GradientStackNode* next;
} GradientStackNode;

/* Stack structure with top pointer only */
typedef struct {
    GradientStackNode* top;
    int64_t size;
    int64_t ref_count;
} GradientStack;

/* Option for (value, stack) tuple used by pop */
typedef struct {
    int64_t tag;              /* 0 = Some, 1 = None */
    struct {
        int64_t value;
        GradientStack* stack;
    } payload;
} OptionInt64Stack;

/*
 * Reference counting functions for GradientStack
 */

/* Increment reference count */
static void stack_retain(GradientStack* s) {
    if (s) {
        s->ref_count++;
    }
}

/* Decrement reference count and free if zero */
static void stack_release(GradientStack* s) {
    if (!s) return;
    s->ref_count--;
    if (s->ref_count <= 0) {
        /* Free all nodes */
        GradientStackNode* node = s->top;
        while (node) {
            GradientStackNode* next = node->next;
            free(node);
            node = next;
        }
        free(s);
    }
}

/* Make a stack mutable: clone if ref_count > 1 (Copy-on-Write) */
static GradientStack* stack_make_mutable(GradientStack* s) {
    if (!s) return NULL;
    if (s->ref_count <= 1) {
        /* Exclusive ownership, can modify directly */
        return s;
    }
    /* Shared copy, need to deep copy */
    s->ref_count--;
    GradientStack* new_s = (GradientStack*)malloc(sizeof(GradientStack));
    if (!new_s) return NULL;
    new_s->size = s->size;
    new_s->ref_count = 1;

    /* Deep copy nodes (stack order: top first) */
    GradientStackNode* new_top = NULL;
    GradientStackNode* cur = s->top;
    while (cur) {
        GradientStackNode* new_node = (GradientStackNode*)malloc(sizeof(GradientStackNode));
        if (!new_node) {
            /* Cleanup on error */
            while (new_top) {
                GradientStackNode* next = new_top->next;
                free(new_top);
                new_top = next;
            }
            free(new_s);
            return NULL;
        }
        new_node->value = cur->value;
        new_node->next = new_top;
        new_top = new_node;
        cur = cur->next;
    }
    new_s->top = new_top;
    return new_s;
}

/* stack_new() -> Stack[T] */
void* __gradient_stack_new(void) {
    GradientStack* s = (GradientStack*)malloc(sizeof(GradientStack));
    if (!s) return NULL;
    s->top = NULL;
    s->size = 0;
    s->ref_count = 1;
    return s;
}

/* stack_push(s: Stack[T], item: T) -> Stack[T] */
/* Note: Uses COW semantics - clones if ref_count > 1 */
void* __gradient_stack_push(GradientStack* s, int64_t item) {
    if (!s) return NULL;

    /* Make mutable (copy if shared) */
    GradientStack* mutable_s = stack_make_mutable(s);
    if (!mutable_s) return NULL;

    /* Create new node */
    GradientStackNode* node = (GradientStackNode*)malloc(sizeof(GradientStackNode));
    if (!node) {
        return NULL;
    }
    node->value = item;

    /* Push at top (LIFO) */
    node->next = mutable_s->top;
    mutable_s->top = node;
    mutable_s->size++;

    return mutable_s;
}

/* stack_pop(s: Stack[T]) -> Option[(T, Stack[T])] */
void* __gradient_stack_pop(GradientStack* s) {
    OptionInt64Stack* opt = (OptionInt64Stack*)malloc(sizeof(OptionInt64Stack));
    if (!opt) return NULL;

    if (!s || !s->top) {
        opt->tag = 1; /* None */
        return opt;
    }

    /* Get the top value */
    int64_t value = s->top->value;

    /* Make mutable (copy if shared) */
    GradientStack* mutable_s = stack_make_mutable(s);
    if (!mutable_s) {
        opt->tag = 1; /* None */
        return opt;
    }

    /* Remove top node */
    GradientStackNode* old_top = mutable_s->top;
    mutable_s->top = old_top->next;
    mutable_s->size--;

    /* Free the removed node */
    free(old_top);

    /* Return Some((value, mutable_s)) */
    opt->tag = 0; /* Some */
    opt->payload.value = value;
    opt->payload.stack = mutable_s;
    return opt;
}

/* stack_peek(s: Stack[T]) -> Option[T] */
void* __gradient_stack_peek(GradientStack* s) {
    OptionInt64Peek* opt = (OptionInt64Peek*)malloc(sizeof(OptionInt64Peek));
    if (!opt) return NULL;

    if (!s || !s->top) {
        opt->tag = 1; /* None */
        return opt;
    }

    opt->tag = 0; /* Some */
    opt->payload = s->top->value;
    return opt;
}

/* stack_size(s: Stack[T]) -> Int */
int64_t __gradient_stack_size(GradientStack* s) {
    if (!s) return 0;
    return s->size;
}

/* ============================================================================
 * Phase PP: String Utilities Batch 2
 * ============================================================================ */

/* string_format(fmt: String, args: List[String]) -> String
 * Simple printf-style formatting supporting %s and %d
 */
char* __gradient_string_format(const char* fmt, int64_t* args) {
    if (!fmt) return __gradient_string_repeat("", 0);
    if (!args) return __gradient_string_repeat("", 0);

    int64_t len = args[0];
    int64_t capacity = args[1];
    char** strings = (char**)(args + 2);

    /* Calculate buffer size */
    size_t fmt_len = strlen(fmt);
    size_t buf_size = fmt_len + 1;

    for (int64_t i = 0; i < len; i++) {
        if (strings[i]) {
            buf_size += strlen(strings[i]);
        }
    }

    char* result = (char*)malloc(buf_size);
    if (!result) return NULL;

    const char* p = fmt;
    char* out = result;
    int64_t arg_idx = 0;

    while (*p) {
        if (p[0] == '%' && p[1]) {
            if (p[1] == 's' || p[1] == 'd') {
                if (arg_idx < len && strings[arg_idx]) {
                    strcpy(out, strings[arg_idx]);
                    out += strlen(strings[arg_idx]);
                }
                arg_idx++;
                p += 2;
                continue;
            }
        }
        *out++ = *p++;
    }
    *out = '\0';

    return result;
}

/* string_is_empty(s: String) -> Bool */
int64_t __gradient_string_is_empty(const char* s) {
    if (!s) return 1;
    return (s[0] == '\0') ? 1 : 0;
}

/* string_reverse(s: String) -> String */
char* __gradient_string_reverse(const char* s) {
    if (!s) return __gradient_string_repeat("", 0);

    size_t len = strlen(s);
    char* result = (char*)malloc(len + 1);
    if (!result) return NULL;

    for (size_t i = 0; i < len; i++) {
        result[i] = s[len - 1 - i];
    }
    result[len] = '\0';

    return result;
}

/* string_compare(a: String, b: String) -> Int
 * Returns negative if a < b, 0 if equal, positive if a > b
 */
int64_t __gradient_string_compare(const char* a, const char* b) {
    if (!a && !b) return 0;
    if (!a) return -1;
    if (!b) return 1;
    return (int64_t)strcmp(a, b);
}

/* OptionInt64Find layout for string_find return */
typedef struct {
    int64_t tag;      /* 0 = Some, 1 = None */
    int64_t value;    /* Valid if tag == 0 */
} OptionInt64Find;

/* string_find(s: String, substr: String) -> Option[Int] */
void* __gradient_string_find(const char* s, const char* substr) {
    OptionInt64Find* opt = (OptionInt64Find*)malloc(sizeof(OptionInt64Find));
    if (!opt) return NULL;

    if (!s || !substr) {
        opt->tag = 1; /* None */
        return opt;
    }

    const char* found = strstr(s, substr);
    if (found) {
        opt->tag = 0; /* Some */
        opt->value = (int64_t)(found - s);
    } else {
        opt->tag = 1; /* None */
    }
    return opt;
}

/* string_slice(s: String, start: Int, end: Int) -> String */
char* __gradient_string_slice(const char* s, int64_t start, int64_t end) {
    if (!s) return __gradient_string_repeat("", 0);

    size_t len = strlen(s);

    /* Clamp indices */
    if (start < 0) start = 0;
    if (end < 0) end = 0;
    if ((size_t)start > len) start = (int64_t)len;
    if ((size_t)end > len) end = (int64_t)len;
    if (start > end) start = end;

    size_t slice_len = (size_t)(end - start);
    char* result = (char*)malloc(slice_len + 1);
    if (!result) return NULL;

    memcpy(result, s + start, slice_len);
    result[slice_len] = '\0';

    return result;
}

/* ============================================================================
 * Actor Runtime System (Phase SS)
 * ============================================================================
 *
 * A complete actor model implementation using POSIX threads (pthreads).
 *
 * Each actor:
 *   - Runs in its own pthread
 *   - Has a private mailbox for receiving messages
 *   - Processes messages sequentially in a loop
 *   - Can send async messages or make sync "ask" requests
 *
 * Components:
 *   - ActorMessage: individual message with name, payload, and reply info
 *   - ActorMailbox: thread-safe message queue with mutex + condvar
 *   - ActorHandle: actor instance with type, state, mailbox, and thread
 *   - ActorSystem: global registry for actor lookup and lifecycle management
 *   - ActorHandler: function pointer for message dispatch
 *
 * Thread Safety:
 *   - All mailbox operations use pthread_mutex_t
 *   - Condition variables handle blocking receive with timeout support
 *   - Ask pattern uses a temporary reply mailbox for sync communication
 */

#include <pthread.h>

/* Maximum lengths for strings */
#define ACTOR_MAX_TYPE_NAME   64
#define ACTOR_MAX_MESSAGE_NAME 64

/* Default mailbox capacity */
#define ACTOR_MAILBOX_DEFAULT_CAPACITY 1024

/* Actor lifecycle states */
#define ACTOR_STATE_INIT      0
#define ACTOR_STATE_RUNNING   1
#define ACTOR_STATE_STOPPING  2
#define ACTOR_STATE_STOPPED   3

/* Message reply states */
#define ACTOR_REPLY_PENDING   0
#define ACTOR_REPLY_READY     1
#define ACTOR_REPLY_CONSUMED  2

/* Maximum number of registered handlers per actor type */
#define ACTOR_MAX_HANDLERS 32

/* Maximum number of actors in the system */
#define ACTOR_MAX_ACTORS 1024

/* ============================================================================
 * Actor Message Structure
 * ============================================================================ */

typedef struct ActorMessage {
    char message_name[ACTOR_MAX_MESSAGE_NAME];
    void* payload;                    /* Message data (owned by message) */
    struct ActorMailbox* reply_to;    /* For ask pattern, NULL for tell */
    int64_t reply_id;                 /* Unique ID for correlating replies */
    struct ActorMessage* next;        /* Linked list for mailbox queue */
} ActorMessage;

/* ============================================================================
 * Actor Mailbox Structure (Thread-Safe Queue)
 * ============================================================================ */

typedef struct ActorMailbox {
    ActorMessage* head;              /* Queue head (oldest message) */
    ActorMessage* tail;              /* Queue tail (newest message) */
    int64_t size;                    /* Current queue size */
    int64_t capacity;                /* Max queue capacity */
    int64_t total_received;          /* Stats: total messages received */
    int64_t total_sent;              /* Stats: total messages sent */
    
    /* Synchronization */
    pthread_mutex_t mutex;
    pthread_cond_t not_empty;        /* Signal when messages arrive */
    pthread_cond_t not_full;         /* Signal when space available */
    
    /* Reply tracking for ask pattern */
    pthread_mutex_t reply_mutex;
    pthread_cond_t reply_ready;      /* Signal when reply arrives */
    void* reply_value;               /* Reply payload */
    int reply_state;                 /* PENDING, READY, or CONSUMED */
    int64_t next_reply_id;           /* Monotonic reply ID counter */
} ActorMailbox;

/* ============================================================================
 * Actor Handler Registration
 * ============================================================================ */

/* Handler function signature: (actor_state, payload, reply_out) -> new_state */
typedef void* (*ActorHandlerFunc)(void* actor_state, void* payload, void** reply_out);

typedef struct ActorHandler {
    char message_name[ACTOR_MAX_MESSAGE_NAME];
    ActorHandlerFunc handler;
} ActorHandler;

typedef struct ActorHandlerRegistry {
    char actor_type[ACTOR_MAX_TYPE_NAME];
    ActorHandler handlers[ACTOR_MAX_HANDLERS];
    int num_handlers;
    /* Lifecycle callbacks */
    void* (*init_state)(void);       /* Create initial actor state */
    void (*destroy_state)(void*);    /* Cleanup actor state */
    struct ActorHandlerRegistry* next;
} ActorHandlerRegistry;

/* ============================================================================
 * Actor Handle (Instance)
 * ============================================================================ */

typedef struct ActorHandle {
    int64_t id;                      /* Unique actor ID */
    char actor_type[ACTOR_MAX_TYPE_NAME];
    int state;                       /* Lifecycle state */
    void* actor_state;               /* Actor-specific state (opaque pointer) */
    ActorMailbox* mailbox;           /* Inbound message queue */
    pthread_t thread;                /* Actor's pthread */
    int64_t messages_processed;      /* Stats */
    struct ActorHandle* next;        /* Linked list for system registry */
} ActorHandle;

/* ============================================================================
 * Actor System (Global Registry)
 * ============================================================================ */

typedef struct ActorSystem {
    /* Actor registry */
    ActorHandle* actors;
    int64_t num_actors;
    int64_t next_actor_id;
    pthread_mutex_t registry_mutex;
    
    /* Handler registry */
    ActorHandlerRegistry* handlers;
    pthread_mutex_t handler_mutex;
    
    /* System state */
    int initialized;
    int shutting_down;
} ActorSystem;

/* Global actor system instance */
static ActorSystem g_actor_system = {0};

/* ============================================================================
 * Mailbox Operations
 * ============================================================================ */

/*
 * __gradient_actor_mailbox_create() -> ActorMailbox*
 *
 * Create a new mailbox with default capacity.
 * Thread-safe from the start.
 */
ActorMailbox* __gradient_actor_mailbox_create(void) {
    ActorMailbox* mb = (ActorMailbox*)malloc(sizeof(ActorMailbox));
    if (!mb) return NULL;
    
    mb->head = NULL;
    mb->tail = NULL;
    mb->size = 0;
    mb->capacity = ACTOR_MAILBOX_DEFAULT_CAPACITY;
    mb->total_received = 0;
    mb->total_sent = 0;
    mb->reply_value = NULL;
    mb->reply_state = ACTOR_REPLY_CONSUMED;
    mb->next_reply_id = 1;
    
    pthread_mutex_init(&mb->mutex, NULL);
    pthread_cond_init(&mb->not_empty, NULL);
    pthread_cond_init(&mb->not_full, NULL);
    pthread_mutex_init(&mb->reply_mutex, NULL);
    pthread_cond_init(&mb->reply_ready, NULL);
    
    return mb;
}

/*
 * mailbox_destroy(mb) - internal cleanup
 */
static void mailbox_destroy(ActorMailbox* mb) {
    if (!mb) return;
    
    /* Drain remaining messages */
    ActorMessage* msg = mb->head;
    while (msg) {
        ActorMessage* next = msg->next;
        if (msg->payload) free(msg->payload);
        free(msg);
        msg = next;
    }
    
    pthread_mutex_destroy(&mb->mutex);
    pthread_cond_destroy(&mb->not_empty);
    pthread_cond_destroy(&mb->not_full);
    pthread_mutex_destroy(&mb->reply_mutex);
    pthread_cond_destroy(&mb->reply_ready);
    
    free(mb);
}

/*
 * mailbox_enqueue(mb, msg, timeout_ms) -> int
 *
 * Enqueue a message with optional timeout.
 * Returns 1 on success, 0 on timeout/failure.
 */
static int mailbox_enqueue(ActorMailbox* mb, ActorMessage* msg, int64_t timeout_ms) {
    if (!mb || !msg) return 0;
    
    pthread_mutex_lock(&mb->mutex);
    
    /* Wait for space if full */
    while (mb->size >= mb->capacity && !g_actor_system.shutting_down) {
        if (timeout_ms < 0) {
            /* Block indefinitely */
            pthread_cond_wait(&mb->not_full, &mb->mutex);
        } else if (timeout_ms == 0) {
            /* Non-blocking */
            pthread_mutex_unlock(&mb->mutex);
            return 0;
        } else {
            /* Timed wait */
            struct timespec ts;
            clock_gettime(CLOCK_REALTIME, &ts);
            ts.tv_sec += timeout_ms / 1000;
            ts.tv_nsec += (timeout_ms % 1000) * 1000000;
            if (ts.tv_nsec >= 1000000000) {
                ts.tv_sec++;
                ts.tv_nsec -= 1000000000;
            }
            int rc = pthread_cond_timedwait(&mb->not_full, &mb->mutex, &ts);
            if (rc == ETIMEDOUT) {
                pthread_mutex_unlock(&mb->mutex);
                return 0;
            }
        }
    }
    
    if (g_actor_system.shutting_down) {
        pthread_mutex_unlock(&mb->mutex);
        return 0;
    }
    
    /* Append to queue */
    msg->next = NULL;
    if (mb->tail) {
        mb->tail->next = msg;
        mb->tail = msg;
    } else {
        mb->head = mb->tail = msg;
    }
    mb->size++;
    mb->total_sent++;
    
    /* Signal waiting receivers */
    pthread_cond_signal(&mb->not_empty);
    
    pthread_mutex_unlock(&mb->mutex);
    return 1;
}

/*
 * mailbox_dequeue(mb, timeout_ms) -> ActorMessage* or NULL
 *
 * Dequeue a message with optional timeout.
 * Returns NULL on timeout or system shutdown.
 */
static ActorMessage* mailbox_dequeue(ActorMailbox* mb, int64_t timeout_ms) {
    if (!mb) return NULL;
    
    pthread_mutex_lock(&mb->mutex);
    
    /* Wait for messages */
    while (mb->size == 0 && !g_actor_system.shutting_down) {
        if (timeout_ms < 0) {
            /* Block indefinitely */
            pthread_cond_wait(&mb->not_empty, &mb->mutex);
        } else if (timeout_ms == 0) {
            /* Non-blocking */
            pthread_mutex_unlock(&mb->mutex);
            return NULL;
        } else {
            /* Timed wait */
            struct timespec ts;
            clock_gettime(CLOCK_REALTIME, &ts);
            ts.tv_sec += timeout_ms / 1000;
            ts.tv_nsec += (timeout_ms % 1000) * 1000000;
            if (ts.tv_nsec >= 1000000000) {
                ts.tv_sec++;
                ts.tv_nsec -= 1000000000;
            }
            int rc = pthread_cond_timedwait(&mb->not_empty, &mb->mutex, &ts);
            if (rc == ETIMEDOUT) {
                pthread_mutex_unlock(&mb->mutex);
                return NULL;
            }
        }
    }
    
    if (mb->size == 0 || g_actor_system.shutting_down) {
        pthread_mutex_unlock(&mb->mutex);
        return NULL;
    }
    
    /* Remove from head */
    ActorMessage* msg = mb->head;
    mb->head = msg->next;
    if (!mb->head) mb->tail = NULL;
    mb->size--;
    mb->total_received++;
    
    msg->next = NULL;
    
    /* Signal waiting senders */
    pthread_cond_signal(&mb->not_full);
    
    pthread_mutex_unlock(&mb->mutex);
    return msg;
}

/*
 * mailbox_try_dequeue(mb) -> ActorMessage* or NULL
 *
 * Non-blocking dequeue attempt.
 */
static ActorMessage* mailbox_try_dequeue(ActorMailbox* mb) {
    return mailbox_dequeue(mb, 0);
}

/*
 * mailbox_peek(mb) -> ActorMessage* or NULL
 *
 * Look at head message without removing (non-destructive).
 */
static ActorMessage* mailbox_peek(ActorMailbox* mb) {
    if (!mb) return NULL;
    
    pthread_mutex_lock(&mb->mutex);
    ActorMessage* msg = mb->head;
    pthread_mutex_unlock(&mb->mutex);
    return msg;
}

/* ============================================================================
 * Handler Registry Operations
 * ============================================================================ */

/*
 * find_handler_registry(actor_type) -> ActorHandlerRegistry* or NULL
 */
static ActorHandlerRegistry* find_handler_registry(const char* actor_type) {
    pthread_mutex_lock(&g_actor_system.handler_mutex);
    
    ActorHandlerRegistry* reg = g_actor_system.handlers;
    while (reg) {
        if (strcmp(reg->actor_type, actor_type) == 0) {
            pthread_mutex_unlock(&g_actor_system.handler_mutex);
            return reg;
        }
        reg = reg->next;
    }
    
    pthread_mutex_unlock(&g_actor_system.handler_mutex);
    return NULL;
}

/*
 * find_handler(registry, message_name) -> ActorHandlerFunc or NULL
 */
static ActorHandlerFunc find_handler(ActorHandlerRegistry* reg, const char* message_name) {
    if (!reg) return NULL;
    
    for (int i = 0; i < reg->num_handlers; i++) {
        if (strcmp(reg->handlers[i].message_name, message_name) == 0) {
            return reg->handlers[i].handler;
        }
    }
    return NULL;
}

/* ============================================================================
 * Actor Thread Main Loop
 * ============================================================================ */

/*
 * actor_thread_main(arg) -> void*
 *
 * Main loop for each actor thread:
 *   1. Block waiting for messages
 *   2. Dispatch to handler
 *   3. Send reply if ask pattern
 *   4. Continue until shutdown
 */
static void* actor_thread_main(void* arg) {
    ActorHandle* actor = (ActorHandle*)arg;
    if (!actor) return NULL;
    
    ActorHandlerRegistry* registry = find_handler_registry(actor->actor_type);
    
    actor->state = ACTOR_STATE_RUNNING;
    
    while (actor->state == ACTOR_STATE_RUNNING && !g_actor_system.shutting_down) {
        /* Block waiting for a message (-1 = infinite timeout) */
        ActorMessage* msg = mailbox_dequeue(actor->mailbox, -1);
        if (!msg) {
            /* Timeout or shutdown signal */
            continue;
        }
        
        actor->messages_processed++;
        
        /* Dispatch to handler */
        ActorHandlerFunc handler = find_handler(registry, msg->message_name);
        
        void* reply_value = NULL;
        void* new_state = actor->actor_state;
        
        if (handler) {
            new_state = handler(actor->actor_state, msg->payload, &reply_value);
        }
        
        /* Update state (handler may have returned new state) */
        actor->actor_state = new_state;
        
        /* Send reply if this was an ask */
        if (msg->reply_to) {
            pthread_mutex_lock(&msg->reply_to->reply_mutex);
            msg->reply_to->reply_value = reply_value;
            msg->reply_to->reply_state = ACTOR_REPLY_READY;
            pthread_cond_signal(&msg->reply_to->reply_ready);
            pthread_mutex_unlock(&msg->reply_to->reply_mutex);
        } else if (reply_value) {
            /* No reply mailbox but handler produced a value - free it */
            free(reply_value);
        }
        
        /* Cleanup message */
        if (msg->payload) free(msg->payload);
        free(msg);
    }
    
    actor->state = ACTOR_STATE_STOPPED;
    return NULL;
}

/* ============================================================================
 * Actor System Public API
 * ============================================================================ */

/*
 * __gradient_actor_system_init() -> int
 *
 * Initialize the global actor system.
 * Must be called before any other actor operations.
 * Returns 1 on success, 0 on failure.
 */
int __gradient_actor_system_init(void) {
    if (g_actor_system.initialized) {
        return 1;  /* Already initialized */
    }
    
    memset(&g_actor_system, 0, sizeof(ActorSystem));
    
    pthread_mutex_init(&g_actor_system.registry_mutex, NULL);
    pthread_mutex_init(&g_actor_system.handler_mutex, NULL);
    
    g_actor_system.next_actor_id = 1;
    g_actor_system.initialized = 1;
    g_actor_system.shutting_down = 0;
    
    return 1;
}

/*
 * __gradient_actor_system_shutdown() -> void
 *
 * Gracefully shut down all actors and cleanup resources.
 */
void __gradient_actor_system_shutdown(void) {
    if (!g_actor_system.initialized) return;
    
    g_actor_system.shutting_down = 1;
    
    /* Signal all actor mailboxes to wake up */
    pthread_mutex_lock(&g_actor_system.registry_mutex);
    
    ActorHandle* actor = g_actor_system.actors;
    while (actor) {
        actor->state = ACTOR_STATE_STOPPING;
        /* Wake up the actor's thread */
        pthread_mutex_lock(&actor->mailbox->mutex);
        pthread_cond_broadcast(&actor->mailbox->not_empty);
        pthread_mutex_unlock(&actor->mailbox->mutex);
        actor = actor->next;
    }
    
    pthread_mutex_unlock(&g_actor_system.registry_mutex);
    
    /* Wait for all actors to stop */
    actor = g_actor_system.actors;
    while (actor) {
        pthread_join(actor->thread, NULL);
        actor = actor->next;
    }
    
    /* Cleanup all actors */
    while (g_actor_system.actors) {
        ActorHandle* next = g_actor_system.actors->next;
        
        ActorHandlerRegistry* registry = find_handler_registry(g_actor_system.actors->actor_type);
        if (registry && registry->destroy_state) {
            registry->destroy_state(g_actor_system.actors->actor_state);
        }
        
        mailbox_destroy(g_actor_system.actors->mailbox);
        free(g_actor_system.actors);
        g_actor_system.actors = next;
        g_actor_system.num_actors--;
    }
    
    /* Cleanup handler registries */
    while (g_actor_system.handlers) {
        ActorHandlerRegistry* next = g_actor_system.handlers->next;
        free(g_actor_system.handlers);
        g_actor_system.handlers = next;
    }
    
    pthread_mutex_destroy(&g_actor_system.registry_mutex);
    pthread_mutex_destroy(&g_actor_system.handler_mutex);
    
    g_actor_system.initialized = 0;
}

/* ============================================================================
 * Actor Spawning
 * ============================================================================ */

/*
 * __gradient_actor_spawn(actor_type_name) -> ActorHandle*
 *
 * Spawn a new actor of the given type.
 * Returns the actor handle, or NULL on error.
 */
ActorHandle* __gradient_actor_spawn(const char* actor_type_name) {
    if (!g_actor_system.initialized) {
        __gradient_actor_system_init();
    }
    
    if (!actor_type_name || strlen(actor_type_name) >= ACTOR_MAX_TYPE_NAME) {
        return NULL;
    }
    
    /* Check for handler registry */
    ActorHandlerRegistry* registry = find_handler_registry(actor_type_name);
    if (!registry) {
        return NULL;  /* Unknown actor type */
    }
    
    /* Create actor handle */
    ActorHandle* actor = (ActorHandle*)malloc(sizeof(ActorHandle));
    if (!actor) return NULL;
    
    memset(actor, 0, sizeof(ActorHandle));
    
    /* Initialize actor */
    strncpy(actor->actor_type, actor_type_name, ACTOR_MAX_TYPE_NAME - 1);
    actor->actor_type[ACTOR_MAX_TYPE_NAME - 1] = '\0';
    actor->state = ACTOR_STATE_INIT;
    
    /* Create mailbox */
    actor->mailbox = __gradient_actor_mailbox_create();
    if (!actor->mailbox) {
        free(actor);
        return NULL;
    }
    
    /* Initialize state */
    if (registry->init_state) {
        actor->actor_state = registry->init_state();
    }
    
    /* Register in system */
    pthread_mutex_lock(&g_actor_system.registry_mutex);
    actor->id = g_actor_system.next_actor_id++;
    actor->next = g_actor_system.actors;
    g_actor_system.actors = actor;
    g_actor_system.num_actors++;
    pthread_mutex_unlock(&g_actor_system.registry_mutex);
    
    /* Create thread */
    if (pthread_create(&actor->thread, NULL, actor_thread_main, actor) != 0) {
        /* Cleanup on failure */
        pthread_mutex_lock(&g_actor_system.registry_mutex);
        g_actor_system.actors = actor->next;
        g_actor_system.num_actors--;
        pthread_mutex_unlock(&g_actor_system.registry_mutex);
        
        mailbox_destroy(actor->mailbox);
        free(actor);
        return NULL;
    }
    
    return actor;
}

/* ============================================================================
 * Actor Messaging
 * ============================================================================ */

/*
 * message_type_to_name(type_id) -> const char*
 *
 * Convert message type ID to message name.
 * Message type IDs are calculated by codegen as:
 *   hash = sum(bytes) + len * 31
 *   type_id = (hash % 1000) + 1
 */
static const char* message_type_to_name(int64_t type_id) {
    switch (type_id) {
        case 384: return "Log";          /* bytes=290, len=3, hash=383 */
        case 190: return "GetPrefix";    /* bytes=910, len=9, hash=1189 */
        case 58:  return "GetCount";     /* bytes=809, len=8, hash=1057 */
        case 46:  return "GetValue";     /* bytes=797, len=8, hash=1045 */
        case 213: return "Increment";    /* bytes=933, len=9, hash=1212 */
        case 199: return "Decrement";    /* bytes=919, len=9, hash=1198 */
        case 529: return "Init";         /* bytes=404, len=4, hash=528, also Pong */
        case 523: return "Ping";         /* bytes=398, len=4, hash=522 */
        case 547: return "Stop";         /* bytes=422, len=4, hash=546 */
        default: return "Unknown";
    }
}

/*
 * __gradient_actor_send(target_id, message_type, payload, payload_size) -> int64_t
 *
 * Send an async message (fire-and-forget) to an actor.
 * Returns 1 on success, 0 on failure.
 *
 * Note: target_id is the ActorHandle* pointer cast to i64 by codegen.
 */
int64_t __gradient_actor_send(int64_t target_id, int64_t message_type, void* payload, int64_t payload_size) {
    /* target_id is the ActorHandle pointer (bitcast from pointer to i64 by codegen) */
    ActorHandle* handle = (ActorHandle*)target_id;
    if (!handle || handle->state == ACTOR_STATE_STOPPING || handle->state == ACTOR_STATE_STOPPED) {
        return 0;
    }

    /* Convert message type to name */
    const char* message_name = message_type_to_name(message_type);
    if (strlen(message_name) >= ACTOR_MAX_MESSAGE_NAME) {
        return 0;
    }

    /* Create message */
    ActorMessage* msg = (ActorMessage*)malloc(sizeof(ActorMessage));
    if (!msg) return 0;

    memset(msg, 0, sizeof(ActorMessage));
    strncpy(msg->message_name, message_name, ACTOR_MAX_MESSAGE_NAME - 1);
    msg->message_name[ACTOR_MAX_MESSAGE_NAME - 1] = '\0';
    msg->reply_to = NULL;  /* Fire-and-forget, no reply */

    /* Copy payload if provided */
    if (payload && payload_size > 0) {
        msg->payload = malloc(payload_size);
        if (msg->payload) {
            memcpy(msg->payload, payload, payload_size);
        }
    } else if (payload) {
        /* Payload is assumed to be a malloc'd pointer */
        msg->payload = payload;
    }

    /* Enqueue with default timeout */
    return mailbox_enqueue(handle->mailbox, msg, -1);
}

/*
 * __gradient_actor_send_copy(handle, message_name, payload, payload_size) -> int
 *
 * Send with automatic payload copying (for stack-allocated data).
 */
int __gradient_actor_send_copy(ActorHandle* handle, const char* message_name, 
                                void* payload, size_t payload_size) {
    if (!handle || !message_name || handle->state != ACTOR_STATE_RUNNING) {
        return 0;
    }
    
    /* Create message */
    ActorMessage* msg = (ActorMessage*)malloc(sizeof(ActorMessage));
    if (!msg) return 0;
    
    memset(msg, 0, sizeof(ActorMessage));
    strncpy(msg->message_name, message_name, ACTOR_MAX_MESSAGE_NAME - 1);
    msg->message_name[ACTOR_MAX_MESSAGE_NAME - 1] = '\0';
    msg->reply_to = NULL;
    
    /* Copy payload */
    if (payload && payload_size > 0) {
        msg->payload = malloc(payload_size);
        if (msg->payload) {
            memcpy(msg->payload, payload, payload_size);
        }
    }
    
    return mailbox_enqueue(handle->mailbox, msg, -1);
}

/*
 * __gradient_actor_ask(target_id, message_type, payload, payload_size) -> void*
 *
 * Send a synchronous request and wait for reply.
 * Creates a temporary mailbox for the reply.
 * Returns the reply value (caller must free), or NULL on timeout/error.
 *
 * Note: target_id is the ActorHandle* pointer cast to i64 by codegen.
 */
void* __gradient_actor_ask(int64_t target_id, int64_t message_type, void* payload, int64_t payload_size) {
    /* target_id is the ActorHandle pointer (bitcast from pointer to i64 by codegen) */
    ActorHandle* handle = (ActorHandle*)target_id;
    if (!handle || handle->state == ACTOR_STATE_STOPPING || handle->state == ACTOR_STATE_STOPPED) {
        return NULL;
    }

    /* Convert message type to name */
    const char* message_name = message_type_to_name(message_type);
    if (strlen(message_name) >= ACTOR_MAX_MESSAGE_NAME) {
        return NULL;
    }

    /* Create temporary reply mailbox */
    ActorMailbox* reply_mb = __gradient_actor_mailbox_create();
    if (!reply_mb) return NULL;

    /* Create message */
    ActorMessage* msg = (ActorMessage*)malloc(sizeof(ActorMessage));
    if (!msg) {
        mailbox_destroy(reply_mb);
        return NULL;
    }

    memset(msg, 0, sizeof(ActorMessage));
    strncpy(msg->message_name, message_name, ACTOR_MAX_MESSAGE_NAME - 1);
    msg->message_name[ACTOR_MAX_MESSAGE_NAME - 1] = '\0';

    /* Copy payload if provided */
    if (payload && payload_size > 0) {
        msg->payload = malloc(payload_size);
        if (msg->payload) {
            memcpy(msg->payload, payload, payload_size);
        }
    } else if (payload) {
        msg->payload = payload;
    }

    msg->reply_to = reply_mb;
    msg->reply_id = reply_mb->next_reply_id++;

    /* Reset reply state */
    reply_mb->reply_state = ACTOR_REPLY_PENDING;
    reply_mb->reply_value = NULL;

    /* Send message with 5 second timeout */
    int64_t timeout_ms = 5000;
    if (!mailbox_enqueue(handle->mailbox, msg, timeout_ms)) {
        free(msg);
        mailbox_destroy(reply_mb);
        return NULL;
    }

    /* Wait for reply */
    void* result = NULL;
    pthread_mutex_lock(&reply_mb->reply_mutex);

    while (reply_mb->reply_state != ACTOR_REPLY_READY && !g_actor_system.shutting_down) {
        struct timespec ts;
        clock_gettime(CLOCK_REALTIME, &ts);
        ts.tv_sec += timeout_ms / 1000;
        ts.tv_nsec += (timeout_ms % 1000) * 1000000;
        if (ts.tv_nsec >= 1000000000) {
            ts.tv_sec++;
            ts.tv_nsec -= 1000000000;
        }
        int rc = pthread_cond_timedwait(&reply_mb->reply_ready, &reply_mb->reply_mutex, &ts);
        if (rc == ETIMEDOUT) {
            pthread_mutex_unlock(&reply_mb->reply_mutex);
            mailbox_destroy(reply_mb);
            return NULL;
        }
    }

    if (reply_mb->reply_state == ACTOR_REPLY_READY) {
        result = reply_mb->reply_value;
    }

    pthread_mutex_unlock(&reply_mb->reply_mutex);

    /* Cleanup reply mailbox */
    mailbox_destroy(reply_mb);

    return result;
}

/* ============================================================================
 * Actor Reply Operations
 * ============================================================================ */

/*
 * __gradient_actor_reply(reply_mailbox, value) -> int
 *
 * Send a reply to the specified reply mailbox.
 * Used internally by handlers when they need to reply to an ask.
 * Returns 1 on success, 0 on failure.
 */
int __gradient_actor_reply(ActorMailbox* reply_mailbox, void* value) {
    if (!reply_mailbox) return 0;
    
    pthread_mutex_lock(&reply_mailbox->reply_mutex);
    reply_mailbox->reply_value = value;
    reply_mailbox->reply_state = ACTOR_REPLY_READY;
    pthread_cond_signal(&reply_mailbox->reply_ready);
    pthread_mutex_unlock(&reply_mailbox->reply_mutex);
    
    return 1;
}

/* ============================================================================
 * Actor Receive Operations
 * ============================================================================ */

/*
 * __gradient_actor_receive(mailbox, timeout_ms) -> ActorMessage* or NULL
 *
 * Blocking receive with timeout support.
 * Returns a message (caller must free payload and message),
 * or NULL on timeout/error.
 */
ActorMessage* __gradient_actor_receive(ActorMailbox* mailbox, int64_t timeout_ms) {
    return mailbox_dequeue(mailbox, timeout_ms);
}

/*
 * __gradient_actor_try_receive(mailbox) -> ActorMessage* or NULL
 *
 * Non-blocking receive attempt.
 */
ActorMessage* __gradient_actor_try_receive(ActorMailbox* mailbox) {
    return mailbox_try_dequeue(mailbox);
}

/* ============================================================================
 * Actor Registration API
 * ============================================================================ */

/*
 * __gradient_actor_register_type(actor_type, init_func, destroy_func) -> int
 *
 * Register a new actor type with the system.
 * Must be called before spawning actors of this type.
 * Returns 1 on success, 0 on failure.
 */
int __gradient_actor_register_type(const char* actor_type,
                                     void* (*init_state)(void),
                                     void (*destroy_state)(void*)) {
    if (!g_actor_system.initialized) {
        __gradient_actor_system_init();
    }
    
    if (!actor_type || strlen(actor_type) >= ACTOR_MAX_TYPE_NAME) {
        return 0;
    }
    
    /* Check if already registered */
    if (find_handler_registry(actor_type)) {
        return 0;
    }
    
    /* Create registry entry */
    ActorHandlerRegistry* reg = (ActorHandlerRegistry*)malloc(sizeof(ActorHandlerRegistry));
    if (!reg) return 0;
    
    memset(reg, 0, sizeof(ActorHandlerRegistry));
    strncpy(reg->actor_type, actor_type, ACTOR_MAX_TYPE_NAME - 1);
    reg->actor_type[ACTOR_MAX_TYPE_NAME - 1] = '\0';
    reg->init_state = init_state;
    reg->destroy_state = destroy_state;
    
    /* Add to registry */
    pthread_mutex_lock(&g_actor_system.handler_mutex);
    reg->next = g_actor_system.handlers;
    g_actor_system.handlers = reg;
    pthread_mutex_unlock(&g_actor_system.handler_mutex);
    
    return 1;
}

/*
 * __gradient_actor_register_handler(actor_type, message_name, handler) -> int
 *
 * Register a message handler for an actor type.
 * The handler receives (state, payload, reply_out) and returns new_state.
 * Returns 1 on success, 0 on failure.
 */
int __gradient_actor_register_handler(const char* actor_type,
                                      const char* message_name,
                                      void* (*handler)(void*, void*, void**)) {
    if (!actor_type || !message_name || !handler) {
        return 0;
    }
    
    if (strlen(message_name) >= ACTOR_MAX_MESSAGE_NAME) {
        return 0;
    }
    
    /* Find or create registry */
    ActorHandlerRegistry* reg = find_handler_registry(actor_type);
    if (!reg) {
        return 0;  /* Type must be registered first */
    }
    
    /* Check for duplicate */
    for (int i = 0; i < reg->num_handlers; i++) {
        if (strcmp(reg->handlers[i].message_name, message_name) == 0) {
            return 0;  /* Already registered */
        }
    }
    
    /* Check capacity */
    if (reg->num_handlers >= ACTOR_MAX_HANDLERS) {
        return 0;
    }
    
    /* Register handler */
    strncpy(reg->handlers[reg->num_handlers].message_name, message_name, ACTOR_MAX_MESSAGE_NAME - 1);
    reg->handlers[reg->num_handlers].message_name[ACTOR_MAX_MESSAGE_NAME - 1] = '\0';
    reg->handlers[reg->num_handlers].handler = handler;
    reg->num_handlers++;
    
    return 1;
}

/* ============================================================================
 * Actor Introspection and Utilities
 * ============================================================================ */

/*
 * __gradient_actor_get_id(handle) -> int64_t
 *
 * Get the unique ID of an actor.
 */
int64_t __gradient_actor_get_id(ActorHandle* handle) {
    if (!handle) return -1;
    return handle->id;
}

/*
 * __gradient_actor_get_type(handle) -> const char*
 *
 * Get the actor type name.
 */
const char* __gradient_actor_get_type(ActorHandle* handle) {
    if (!handle) return NULL;
    return handle->actor_type;
}

/*
 * __gradient_actor_get_state(handle) -> int
 *
 * Get the actor lifecycle state.
 */
int __gradient_actor_get_state(ActorHandle* handle) {
    if (!handle) return ACTOR_STATE_STOPPED;
    return handle->state;
}

/*
 * __gradient_actor_mailbox_size(handle) -> int64_t
 *
 * Get the number of pending messages in an actor's mailbox.
 */
int64_t __gradient_actor_mailbox_size(ActorHandle* handle) {
    if (!handle || !handle->mailbox) return -1;
    
    pthread_mutex_lock(&handle->mailbox->mutex);
    int64_t size = handle->mailbox->size;
    pthread_mutex_unlock(&handle->mailbox->mutex);
    
    return size;
}

/*
 * __gradient_actor_messages_processed(handle) -> int64_t
 *
 * Get the total number of messages processed by an actor.
 */
int64_t __gradient_actor_messages_processed(ActorHandle* handle) {
    if (!handle) return -1;
    return handle->messages_processed;
}

/*
 * __gradient_actor_count() -> int64_t
 *
 * Get the total number of actors in the system.
 */
int64_t __gradient_actor_count(void) {
    if (!g_actor_system.initialized) return 0;
    
    pthread_mutex_lock(&g_actor_system.registry_mutex);
    int64_t count = g_actor_system.num_actors;
    pthread_mutex_unlock(&g_actor_system.registry_mutex);
    
    return count;
}

/*
 * __gradient_actor_stop(handle) -> int
 *
 * Gracefully stop an actor.
 * Returns 1 on success, 0 on failure.
 */
int __gradient_actor_stop(ActorHandle* handle) {
    if (!handle || handle->state != ACTOR_STATE_RUNNING) {
        return 0;
    }
    
    handle->state = ACTOR_STATE_STOPPING;
    
    /* Wake up the actor thread */
    pthread_mutex_lock(&handle->mailbox->mutex);
    pthread_cond_broadcast(&handle->mailbox->not_empty);
    pthread_mutex_unlock(&handle->mailbox->mutex);
    
    /* Wait for thread to finish */
    pthread_join(handle->thread, NULL);
    
    return 1;
}

/* ============================================================================
 * Actor Message Utilities
 * ============================================================================ */

/*
 * __gradient_actor_message_name(msg) -> const char*
 *
 * Get the message name from a received message.
 */
const char* __gradient_actor_message_name(ActorMessage* msg) {
    if (!msg) return NULL;
    return msg->message_name;
}

/*
 * __gradient_actor_message_payload(msg) -> void*
 *
 * Get the payload from a received message.
 * The payload is transferred to caller ownership.
 */
void* __gradient_actor_message_payload(ActorMessage* msg) {
    if (!msg) return NULL;
    void* payload = msg->payload;
    msg->payload = NULL;  /* Transfer ownership */
    return payload;
}

/*
 * __gradient_actor_message_reply_to(msg) -> ActorMailbox*
 *
 * Get the reply mailbox from a message (NULL for tell messages).
 */
ActorMailbox* __gradient_actor_message_reply_to(ActorMessage* msg) {
    if (!msg) return NULL;
    return msg->reply_to;
}

/*
 * __gradient_actor_message_free(msg) -> void
 *
 * Free a message and its resources.
 */
void __gradient_actor_message_free(ActorMessage* msg) {
    if (!msg) return;
    if (msg->payload) free(msg->payload);
    free(msg);
}

/* ============================================================================
 * Actor State Helpers
 * ============================================================================ */

/*
 * __gradient_actor_set_state(handle, new_state) -> void*
 *
 * Update an actor's state (thread-safe, only call from actor thread).
 * Returns the old state (caller should free if needed).
 */
void* __gradient_actor_set_state(ActorHandle* handle, void* new_state) {
    if (!handle) return NULL;
    void* old_state = handle->actor_state;
    handle->actor_state = new_state;
    return old_state;
}

/*
 * __gradient_actor_get_state_ptr(handle) -> void*
 *
 * Get the current actor state pointer (for reading).
 */
void* __gradient_actor_get_state_ptr(ActorHandle* handle) {
    if (!handle) return NULL;
    return handle->actor_state;
}

/* ============================================================================
 * Arena Allocator Runtime
 * ============================================================================
 *
 * Bump-pointer arena allocator for efficient temporary memory management.
 * Used by Gradient's 'defer' and arena-allocation syntax.
 */

/* Default chunk size: 64KB */
#define ARENA_DEFAULT_CHUNK_SIZE (64 * 1024)

/* Minimum chunk size: 4KB */
#define ARENA_MIN_CHUNK_SIZE (4 * 1024)

/* Chunk node in the arena's linked list of chunks */
typedef struct ArenaChunk {
    struct ArenaChunk* next;   /* Next chunk in list */
    size_t size;               /* Total size of this chunk */
    size_t used;               /* Bytes used in this chunk */
    uint8_t data[];            /* Flexible array member for data */
} ArenaChunk;

/* Arena structure with bump pointer allocation */
typedef struct Arena {
    ArenaChunk* chunks;        /* Linked list of chunks (head = current) */
    uint8_t* bump_ptr;         /* Current bump pointer */
    uint8_t* end_ptr;          /* End of current chunk */
    size_t chunk_size;         /* Default size for new chunks */
    size_t total_allocated;    /* Total bytes allocated across all chunks */
    int num_chunks;            /* Number of chunks allocated */
} Arena;

/* Internal: Allocate a new chunk */
static ArenaChunk* arena_chunk_new(size_t chunk_size) {
    size_t total_size = sizeof(ArenaChunk) + chunk_size;
    ArenaChunk* chunk = (ArenaChunk*)malloc(total_size);
    if (!chunk) return NULL;
    
    chunk->next = NULL;
    chunk->size = chunk_size;
    chunk->used = 0;
    return chunk;
}

/* Internal: Free all chunks in a linked list */
static void arena_chunks_free(ArenaChunk* chunk) {
    while (chunk) {
        ArenaChunk* next = chunk->next;
        free(chunk);
        chunk = next;
    }
}

/* Round up to nearest multiple of alignment (must be power of 2) */
static inline uintptr_t align_ptr_up(uintptr_t ptr, size_t align) {
    return (ptr + align - 1) & ~(align - 1);
}

/*
 * __gradient_arena_create() -> Arena*
 *
 * Create a new arena allocator with default chunk size (64KB).
 * Returns pointer to arena on success, NULL on failure.
 */
void* __gradient_arena_create(void) {
    size_t chunk_size = ARENA_DEFAULT_CHUNK_SIZE;
    
    Arena* arena = (Arena*)malloc(sizeof(Arena));
    if (!arena) return NULL;
    
    ArenaChunk* chunk = arena_chunk_new(chunk_size);
    if (!chunk) {
        free(arena);
        return NULL;
    }
    
    arena->chunks = chunk;
    arena->bump_ptr = chunk->data;
    arena->end_ptr = chunk->data + chunk_size;
    arena->chunk_size = chunk_size;
    arena->total_allocated = 0;
    arena->num_chunks = 1;
    
    return arena;
}

/*
 * __gradient_arena_alloc(arena, size) -> void*
 *
 * Allocate `size` bytes from the arena with 8-byte alignment.
 * Returns pointer to allocated memory, or NULL on failure.
 * Memory is zero-initialized.
 */
void* __gradient_arena_alloc(void* arena_ptr, int64_t size) {
    if (!arena_ptr || size <= 0) return NULL;
    
    Arena* arena = (Arena*)arena_ptr;
    size_t align = 8;
    size_t sz = (size_t)size;
    
    /* Try to allocate from current chunk */
    uintptr_t current = (uintptr_t)arena->bump_ptr;
    uintptr_t aligned = align_ptr_up(current, align);
    size_t padding = aligned - current;
    
    /* Check if there's enough space in current chunk */
    if (aligned + sz <= (uintptr_t)arena->end_ptr) {
        void* result = (void*)aligned;
        arena->bump_ptr = (uint8_t*)(aligned + sz);
        arena->chunks->used += padding + sz;
        arena->total_allocated += sz;
        memset(result, 0, sz);
        return result;
    }
    
    /* Need a new chunk */
    size_t required_size = sz + align;
    size_t new_chunk_size = arena->chunk_size;
    if (required_size > new_chunk_size) {
        new_chunk_size = required_size;
    }
    
    ArenaChunk* new_chunk = arena_chunk_new(new_chunk_size);
    if (!new_chunk) return NULL;
    
    /* Link new chunk to front of list */
    new_chunk->next = arena->chunks;
    arena->chunks = new_chunk;
    arena->num_chunks++;
    
    /* Set up bump pointer in new chunk */
    arena->bump_ptr = new_chunk->data;
    arena->end_ptr = new_chunk->data + new_chunk_size;
    
    /* Allocate from new chunk */
    current = (uintptr_t)arena->bump_ptr;
    aligned = align_ptr_up(current, align);
    padding = aligned - current;
    
    void* result = (void*)aligned;
    arena->bump_ptr = (uint8_t*)(aligned + sz);
    new_chunk->used = padding + sz;
    arena->total_allocated += sz;
    memset(result, 0, sz);
    
    return result;
}

/*
 * __gradient_arena_dealloc_all(arena) -> void
 *
 * Reset the arena, freeing all chunks except keeping one empty chunk
 * for reuse. This effectively clears all allocations.
 */
void __gradient_arena_dealloc_all(void* arena_ptr) {
    if (!arena_ptr) return;
    
    Arena* arena = (Arena*)arena_ptr;
    
    /* Keep the first chunk for reuse, free the rest */
    ArenaChunk* first = arena->chunks;
    if (!first) return;
    
    ArenaChunk* rest = first->next;
    
    /* Reset first chunk */
    first->next = NULL;
    first->used = 0;
    
    /* Free remaining chunks */
    arena_chunks_free(rest);
    
    /* Reset arena state */
    arena->bump_ptr = first->data;
    arena->end_ptr = first->data + first->size;
    arena->total_allocated = 0;
    arena->num_chunks = 1;
}

/*
 * __gradient_arena_destroy(arena) -> void
 *
 * Destroy the arena and free all associated memory.
 */
void __gradient_arena_destroy(void* arena_ptr) {
    if (!arena_ptr) return;
    
    Arena* arena = (Arena*)arena_ptr;
    arena_chunks_free(arena->chunks);
    free(arena);
}

/* ============================================================================
 * Generational References Runtime
 * ============================================================================
 *
 * Tier 2 memory model: mutable aliasing with generation tracking.
 * Each allocation has a monotonically increasing generation counter.
 * References store (ptr, generation) and check on dereference.
 *
 * This enables safe shared mutable state without garbage collection
 * or full borrow checking. It's particularly useful for:
 * - Graph structures with cycles
 * - Observer patterns
 * - Cache invalidation detection
 * - Concurrent data structures
 */

/*
 * GenRef: A generational reference (pointer + generation)
 */
typedef struct GenRef {
    void* ptr;              /* Pointer to the allocation */
    uint64_t generation;    /* Generation at time of reference creation */
} GenRef;

/*
 * GenHeader: Header stored before user data in genref allocations
 */
typedef struct GenHeader {
    uint64_t generation;    /* Current generation, incremented on update/free */
    size_t size;          /* Size of user allocation (for debugging) */
    uint32_t magic;       /* Magic number for header validation */
} GenHeader;

/* Magic number for header validation: "GENR" in hex */
#define GENREF_MAGIC 0x47454E52

/* Size of the header that precedes user data */
#define GENREF_HEADER_SIZE sizeof(GenHeader)

/*
 * Get the header pointer from user data pointer.
 * The header is stored immediately before the user data.
 */
static GenHeader* genref_get_header(void* ptr) {
    if (!ptr) return NULL;
    return (GenHeader*)((uint8_t*)ptr - GENREF_HEADER_SIZE);
}

/*
 * Validate that a pointer is a valid genref allocation.
 */
static int is_valid_genref(void* ptr) {
    if (!ptr) return 0;
    GenHeader* header = genref_get_header(ptr);
    return header->magic == GENREF_MAGIC;
}

/*
 * __gradient_genref_alloc(size) -> void*
 *
 * Allocate memory with generation tracking. The returned pointer points to
 * the user-visible data (after the internal GenHeader).
 *
 * The allocation starts at generation 1. Use genref_new() to create
 * a GenRef pointing to this allocation.
 *
 * Returns NULL on allocation failure.
 */
void* __gradient_genref_alloc(int64_t size) {
    if (size <= 0) return NULL;
    
    /* Allocate space for header + user data */
    size_t total_size = GENREF_HEADER_SIZE + (size_t)size;
    void* mem = malloc(total_size);
    if (!mem) return NULL;
    
    /* Initialize header */
    GenHeader* header = (GenHeader*)mem;
    header->generation = 1;  /* Start at generation 1 */
    header->size = (size_t)size;
    header->magic = GENREF_MAGIC;
    
    /* Return pointer to user data (after header) */
    void* user_ptr = (uint8_t*)mem + GENREF_HEADER_SIZE;
    
    /* Zero-initialize user data */
    memset(user_ptr, 0, (size_t)size);
    
    return user_ptr;
}

/*
 * __gradient_genref_free(ptr) -> void
 *
 * Free memory allocated with genref_alloc().
 * This also invalidates all existing GenRefs by incrementing the generation.
 */
void __gradient_genref_free(void* ptr) {
    if (!ptr) return;
    
    /* Validate this is actually a genref allocation */
    GenHeader* header = genref_get_header(ptr);
    if (header->magic != GENREF_MAGIC) {
        /* Not a valid genref - ignore or could assert in debug builds */
        return;
    }
    
    /* Increment generation to invalidate all existing references */
    header->generation++;
    
    /* Clear magic to mark as freed */
    header->magic = 0;
    
    /* Free the entire block including header */
    free(header);
}

/*
 * __gradient_genref_new(ptr) -> GenRef
 *
 * Create a GenRef pointing to an allocation made with genref_alloc().
 * Captures the current generation of the allocation.
 */
GenRef __gradient_genref_new(void* ptr) {
    GenRef ref = { NULL, 0 };
    
    if (!ptr) return ref;
    
    GenHeader* header = genref_get_header(ptr);
    if (header->magic != GENREF_MAGIC) {
        /* Not a valid genref allocation */
        return ref;
    }
    
    ref.ptr = ptr;
    ref.generation = header->generation;
    return ref;
}

/*
 * __gradient_genref_get(ref) -> void*
 *
 * Validate and dereference a GenRef. Checks if the stored generation
 * matches the allocation's current generation.
 *
 * Returns the pointer if valid, NULL if the reference is stale
 * (generation mismatch indicates the allocation was updated/reused).
 */
void* __gradient_genref_get(GenRef ref) {
    if (!ref.ptr) return NULL;
    
    GenHeader* header = genref_get_header(ref.ptr);
    if (header->magic != GENREF_MAGIC) {
        /* Allocation was freed or corrupted */
        return NULL;
    }
    
    /* Check generation match */
    if (header->generation != ref.generation) {
        /* Reference is stale - allocation was updated */
        return NULL;
    }
    
    return ref.ptr;
}

/*
 * __gradient_genref_set(ref, new_ptr) -> int64_t
 *
 * Update the allocation pointed to by ref to point to new_ptr.
 * This operation:
 * 1. Validates ref's generation against the allocation
 * 2. If valid, increments the allocation's generation
 * 3. Updates the allocation to point to new data (copies content)
 * 4. Returns 1 on success, 0 on failure (stale reference)
 *
 * After this call, all existing GenRefs to the allocation become stale.
 */
int64_t __gradient_genref_set(GenRef ref, void* new_ptr) {
    if (!ref.ptr || !new_ptr) return 0;
    
    GenHeader* header = genref_get_header(ref.ptr);
    if (header->magic != GENREF_MAGIC) {
        /* Not a valid genref or already freed */
        return 0;
    }
    
    /* Validate ref's generation matches current */
    if (header->generation != ref.generation) {
        /* Reference is stale */
        return 0;
    }
    
    /* Increment generation - invalidates all existing references */
    header->generation++;
    
    /* Copy content from new_ptr to the allocation */
    memcpy(ref.ptr, new_ptr, header->size);
    
    return 1;
}

/*
 * __gradient_genref_get_generation(ptr) -> uint64_t
 *
 * Get the current generation of an allocation.
 * Returns 0 if ptr is not a valid genref allocation.
 */
uint64_t __gradient_genref_get_generation(void* ptr) {
    if (!ptr) return 0;
    
    GenHeader* header = genref_get_header(ptr);
    if (header->magic != GENREF_MAGIC) {
        return 0;
    }
    
    return header->generation;
}

/*
 * __gradient_genref_is_valid(ref) -> int64_t
 *
 * Check if a GenRef is still valid (generation matches).
 * Returns 1 if valid, 0 if stale.
 */
int64_t __gradient_genref_is_valid(GenRef ref) {
    if (!ref.ptr) return 0;
    
    GenHeader* header = genref_get_header(ref.ptr);
    if (header->magic != GENREF_MAGIC) {
        return 0;
    }
    
    return header->generation == ref.generation ? 1 : 0;
}

/*
 * Note: The actor runtime is implemented in runtime/vm/actor.c and scheduler.c
 * with a work-stealing thread pool scheduler. The legacy actor runtime below
 * provides an alternative implementation using per-actor threads.
 *
 * New code should use the runtime/vm/ implementation via the functions
 * declared in runtime/vm/actor.h and runtime/vm/scheduler.h.
 */

/*
 * ============================================================================
 * Self-Hosting Phase 1.1: HashMap with Generic Keys
 * ============================================================================
 *
 * HashMap[K, V] provides a hash table with O(1) average-case operations
 * for arbitrary key types. Uses separate chaining for collision resolution.
 *
 * The current implementation supports:
 *   - String keys: Uses FNV-1a hash
 *   - Integer keys: Uses identity hash
 *   - Generic values: Stored as opaque pointers (i64)
 *
 * Layout:
 *   typedef struct {
 *       int64_t  size;        // Number of entries
 *       int64_t  capacity;    // Number of buckets
 *       int64_t  key_type;    // 0=String, 1=Int (for hash/compare)
 *       void**   buckets;     // Array of GradientHashEntry* (linked lists)
 *       int      ref_count;   // For COW semantics
 *   } GradientHashMap;
 *
 * Entry layout (separate chaining):
 *   typedef struct GradientHashEntry {
 *       uint32_t hash;              // Cached hash value
 *       void*    key;               // Key (String or boxed Int)
 *       int64_t  value;             // Value (generic)
 *       struct GradientHashEntry* next;
 *   } GradientHashEntry;
 */

#define HASHMAP_DEFAULT_CAPACITY 16
#define HASHMAP_LOAD_FACTOR 0.75

/* HashMap entry (node in separate chaining linked list) */
typedef struct GradientHashEntry {
    uint32_t hash;                 // Cached hash value
    void*    key;                 // Key pointer (owned by entry)
    int64_t  value;               // Value (generic, may be pointer)
    struct GradientHashEntry* next;
} GradientHashEntry;

/* HashMap key type identifiers */
#define HASHMAP_KEY_STRING 0
#define HASHMAP_KEY_INT   1

/* HashMap structure */
typedef struct {
    int64_t  size;                // Number of entries
    int64_t  capacity;            // Number of buckets
    int64_t  key_type;            // HASHMAP_KEY_STRING or HASHMAP_KEY_INT
    void**   buckets;             // Array of GradientHashEntry* (linked lists)
    int      ref_count;           // Reference count for COW
} GradientHashMap;

/*
 * FNV-1a 32-bit hash function for strings
 */
static uint32_t fnv1a_32_string(const char* str) {
    uint32_t hash = 0x811c9dc5u;  // FNV offset basis
    uint32_t prime = 0x01000193u; // FNV prime

    for (const char* p = str; *p; p++) {
        hash ^= (uint8_t)*p;
        hash *= prime;
    }
    return hash;
}

/*
 * Hash function for integers (just use the value)
 */
static uint32_t hash_int(int64_t value) {
    // Mix the bits for better distribution
    uint64_t v = (uint64_t)value;
    v = (v ^ (v >> 33)) * 0xff51afd7ed558ccdull;
    v = (v ^ (v >> 33)) * 0xc4ceb9fe1a85ec53ull;
    v = v ^ (v >> 33);
    return (uint32_t)v;
}

/*
 * Compare two keys for equality
 */
static int hashmap_keys_equal(int key_type, void* key1, void* key2) {
    if (key_type == HASHMAP_KEY_STRING) {
        return strcmp((char*)key1, (char*)key2) == 0;
    } else {
        return *(int64_t*)key1 == *(int64_t*)key2;
    }
}

/*
 * Free a key
 */
static void hashmap_free_key(int key_type, void* key) {
    if (key_type == HASHMAP_KEY_STRING) {
        free(key);
    } else {
        free(key); // Boxed int
    }
}

/*
 * Duplicate a key
 */
static void* hashmap_dup_key(int key_type, void* key) {
    if (key_type == HASHMAP_KEY_STRING) {
        return strdup((char*)key);
    } else {
        int64_t* boxed = (int64_t*)malloc(sizeof(int64_t));
        *boxed = *(int64_t*)key;
        return boxed;
    }
}

/*
 * Allocate a new HashMap with given capacity and key type
 */
static GradientHashMap* hashmap_alloc(int64_t capacity, int key_type) {
    GradientHashMap* hm = (GradientHashMap*)malloc(sizeof(GradientHashMap));
    hm->size = 0;
    hm->capacity = capacity;
    hm->key_type = key_type;
    hm->buckets = (void**)calloc((size_t)capacity, sizeof(void*));
    hm->ref_count = 1;
    return hm;
}

/*
 * Free a hash entry and its key
 */
static void hashmap_free_entry(GradientHashEntry* entry, int key_type) {
    if (!entry) return;
    hashmap_free_key(key_type, entry->key);
    free(entry);
}

/*
 * Free all entries in a bucket chain
 */
static void hashmap_free_chain(GradientHashEntry* entry, int key_type) {
    while (entry) {
        GradientHashEntry* next = entry->next;
        hashmap_free_entry(entry, key_type);
        entry = next;
    }
}

/*
 * hashmap_retain: Increment reference count
 */
static GradientHashMap* hashmap_retain(GradientHashMap* hm) {
    if (!hm) return NULL;
    hm->ref_count++;
    return hm;
}

/*
 * hashmap_release: Decrement reference count and free if 0
 */
static void hashmap_release(GradientHashMap* hm) {
    if (!hm) return;
    hm->ref_count--;
    if (hm->ref_count <= 0) {
        // Free all buckets
        for (int64_t i = 0; i < hm->capacity; i++) {
            hashmap_free_chain(hm->buckets[i], (int)hm->key_type);
        }
        free(hm->buckets);
        free(hm);
    }
}

/*
 * Resize the hashmap when load factor exceeded
 */
static GradientHashMap* hashmap_resize(GradientHashMap* hm) {
    int64_t new_cap = hm->capacity * 2;
    void** new_buckets = (void**)calloc((size_t)new_cap, sizeof(void*));

    // Rehash all entries
    for (int64_t i = 0; i < hm->capacity; i++) {
        GradientHashEntry* entry = (GradientHashEntry*)hm->buckets[i];
        while (entry) {
            GradientHashEntry* next = entry->next;

            // Compute new bucket index
            int64_t new_idx = (int64_t)(entry->hash % (uint32_t)new_cap);

            // Move to new bucket
            entry->next = (GradientHashEntry*)new_buckets[new_idx];
            new_buckets[new_idx] = entry;

            entry = next;
        }
    }

    free(hm->buckets);
    hm->buckets = new_buckets;
    hm->capacity = new_cap;
    return hm;
}

/*
 * __gradient_hashmap_new_string() -> GradientHashMap*
 *
 * Create a new HashMap with String keys.
 */
void* __gradient_hashmap_new_string(void) {
    return (void*)hashmap_alloc(HASHMAP_DEFAULT_CAPACITY, HASHMAP_KEY_STRING);
}

/*
 * __gradient_hashmap_new_int() -> GradientHashMap*
 *
 * Create a new HashMap with Int keys.
 */
void* __gradient_hashmap_new_int(void) {
    return (void*)hashmap_alloc(HASHMAP_DEFAULT_CAPACITY, HASHMAP_KEY_INT);
}

/*
 * __gradient_hashmap_insert_string(hm, key, value) -> Option[Int]
 *
 * Insert a key-value pair into a String-keyed HashMap.
 * Returns the old value if the key existed, or None (0 with special encoding).
 *
 * For the Option return, we use: 0 = None, non-zero = Some(ptr)
 * The caller must check the discriminant.
 */
void* __gradient_hashmap_insert_string(void* hm, char* key, int64_t value) {
    GradientHashMap* map = (GradientHashMap*)hm;
    if (!map) return NULL;

    // Check load factor
    if ((double)map->size / (double)map->capacity > HASHMAP_LOAD_FACTOR) {
        hashmap_resize(map);
    }

    uint32_t hash = fnv1a_32_string(key);
    int64_t idx = (int64_t)(hash % (uint32_t)map->capacity);

    // Check if key already exists
    GradientHashEntry* entry = (GradientHashEntry*)map->buckets[idx];
    while (entry) {
        if (entry->hash == hash && strcmp((char*)entry->key, key) == 0) {
            // Key exists - update value and return old
            int64_t old_value = entry->value;
            entry->value = value;

            // Return Some(old_value) - boxed for uniformity
            int64_t* result = (int64_t*)malloc(sizeof(int64_t));
            *result = old_value;
            return result;
        }
        entry = entry->next;
    }

    // Key doesn't exist - create new entry
    GradientHashEntry* new_entry = (GradientHashEntry*)malloc(sizeof(GradientHashEntry));
    new_entry->hash = hash;
    new_entry->key = strdup(key);
    new_entry->value = value;
    new_entry->next = (GradientHashEntry*)map->buckets[idx];
    map->buckets[idx] = new_entry;
    map->size++;

    // Return None (represented as NULL - caller must handle)
    return NULL;
}

/*
 * __gradient_hashmap_insert_int(hm, key, value) -> Option[Int]
 *
 * Insert into an Int-keyed HashMap.
 */
void* __gradient_hashmap_insert_int(void* hm, int64_t key, int64_t value) {
    GradientHashMap* map = (GradientHashMap*)hm;
    if (!map) return NULL;

    if ((double)map->size / (double)map->capacity > HASHMAP_LOAD_FACTOR) {
        hashmap_resize(map);
    }

    uint32_t hash = hash_int(key);
    int64_t idx = (int64_t)(hash % (uint32_t)map->capacity);

    GradientHashEntry* entry = (GradientHashEntry*)map->buckets[idx];
    while (entry) {
        if (entry->hash == hash && *(int64_t*)entry->key == key) {
            int64_t old_value = entry->value;
            entry->value = value;
            int64_t* result = (int64_t*)malloc(sizeof(int64_t));
            *result = old_value;
            return result;
        }
        entry = entry->next;
    }

    GradientHashEntry* new_entry = (GradientHashEntry*)malloc(sizeof(GradientHashEntry));
    new_entry->hash = hash;
    new_entry->key = malloc(sizeof(int64_t));
    *(int64_t*)new_entry->key = key;
    new_entry->value = value;
    new_entry->next = (GradientHashEntry*)map->buckets[idx];
    map->buckets[idx] = new_entry;
    map->size++;

    return NULL;
}

/*
 * __gradient_hashmap_get_string(hm, key) -> Option[Int]
 *
 * Get a value from a String-keyed HashMap.
 * Returns NULL for None, or a pointer to the value for Some.
 */
void* __gradient_hashmap_get_string(void* hm, char* key) {
    GradientHashMap* map = (GradientHashMap*)hm;
    if (!map) return NULL;

    uint32_t hash = fnv1a_32_string(key);
    int64_t idx = (int64_t)(hash % (uint32_t)map->capacity);

    GradientHashEntry* entry = (GradientHashEntry*)map->buckets[idx];
    while (entry) {
        if (entry->hash == hash && strcmp((char*)entry->key, key) == 0) {
            // Found - return pointer to value
            return &entry->value;
        }
        entry = entry->next;
    }

    return NULL;
}

/*
 * __gradient_hashmap_get_int(hm, key) -> Option[Int]
 */
void* __gradient_hashmap_get_int(void* hm, int64_t key) {
    GradientHashMap* map = (GradientHashMap*)hm;
    if (!map) return NULL;

    uint32_t hash = hash_int(key);
    int64_t idx = (int64_t)(hash % (uint32_t)map->capacity);

    GradientHashEntry* entry = (GradientHashEntry*)map->buckets[idx];
    while (entry) {
        if (entry->hash == hash && *(int64_t*)entry->key == key) {
            return &entry->value;
        }
        entry = entry->next;
    }

    return NULL;
}

/*
 * __gradient_hashmap_remove_string(hm, key) -> Option[Int]
 *
 * Remove a key and return its value if it existed.
 */
void* __gradient_hashmap_remove_string(void* hm, char* key) {
    GradientHashMap* map = (GradientHashMap*)hm;
    if (!map) return NULL;

    uint32_t hash = fnv1a_32_string(key);
    int64_t idx = (int64_t)(hash % (uint32_t)map->capacity);

    GradientHashEntry** current = (GradientHashEntry**)&map->buckets[idx];
    while (*current) {
        GradientHashEntry* entry = *current;
        if (entry->hash == hash && strcmp((char*)entry->key, key) == 0) {
            // Found - unlink and return value
            *current = entry->next;
            int64_t* result = (int64_t*)malloc(sizeof(int64_t));
            *result = entry->value;
            hashmap_free_entry(entry, HASHMAP_KEY_STRING);
            map->size--;
            return result;
        }
        current = &entry->next;
    }

    return NULL;
}

/*
 * __gradient_hashmap_remove_int(hm, key) -> Option[Int]
 */
void* __gradient_hashmap_remove_int(void* hm, int64_t key) {
    GradientHashMap* map = (GradientHashMap*)hm;
    if (!map) return NULL;

    uint32_t hash = hash_int(key);
    int64_t idx = (int64_t)(hash % (uint32_t)map->capacity);

    GradientHashEntry** current = (GradientHashEntry**)&map->buckets[idx];
    while (*current) {
        GradientHashEntry* entry = *current;
        if (entry->hash == hash && *(int64_t*)entry->key == key) {
            *current = entry->next;
            int64_t* result = (int64_t*)malloc(sizeof(int64_t));
            *result = entry->value;
            hashmap_free_entry(entry, HASHMAP_KEY_INT);
            map->size--;
            return result;
        }
        current = &entry->next;
    }

    return NULL;
}

/*
 * __gradient_hashmap_contains_string(hm, key) -> Int
 *
 * Check if key exists (1 = true, 0 = false).
 */
int64_t __gradient_hashmap_contains_string(void* hm, char* key) {
    return __gradient_hashmap_get_string(hm, key) != NULL ? 1 : 0;
}

/*
 * __gradient_hashmap_contains_int(hm, key) -> Int
 */
int64_t __gradient_hashmap_contains_int(void* hm, int64_t key) {
    return __gradient_hashmap_get_int(hm, key) != NULL ? 1 : 0;
}

/*
 * __gradient_hashmap_len(hm) -> Int
 *
 * Return the number of entries.
 */
int64_t __gradient_hashmap_len(void* hm) {
    GradientHashMap* map = (GradientHashMap*)hm;
    if (!map) return 0;
    return map->size;
}

/*
 * __gradient_hashmap_clear(hm) -> Int
 *
 * Clear all entries. Returns 0 (unit).
 */
int64_t __gradient_hashmap_clear(void* hm) {
    GradientHashMap* map = (GradientHashMap*)hm;
    if (!map) return 0;

    for (int64_t i = 0; i < map->capacity; i++) {
        hashmap_free_chain(map->buckets[i], (int)map->key_type);
        map->buckets[i] = NULL;
    }
    map->size = 0;
    return 0;
}

/*
 * __gradient_hashmap_retain(hm) -> hm
 *
 * Increment reference count (for COW).
 */
void* __gradient_hashmap_retain(void* hm) {
    return hashmap_retain((GradientHashMap*)hm);
}

/*
 * __gradient_hashmap_release(hm)
 *
 * Decrement reference count and free if 0.
 */
void __gradient_hashmap_release(void* hm) {
    hashmap_release((GradientHashMap*)hm);
}

/*
 * ============================================================================
 * Self-Hosting Phase 1.2: Iterator Protocol Runtime
 * ============================================================================
 *
 * Iterator implementations for List and Range types.
 * Supports lazy iteration with the core protocol:
 *   - iter_next() -> Option[T]
 *   - iter_has_next() -> Bool
 *   - iter_count() -> Int (eager consumption)
 *
 * Layout:
 *   GradientListIter: { list_ptr, index, len, ref_count }
 *   GradientRangeIter: { current, end, ref_count }
 */

/* Iterator type tags */
#define ITER_TYPE_LIST  0
#define ITER_TYPE_RANGE 1

/* List iterator */
typedef struct {
    int     iter_type;      // ITER_TYPE_LIST
    void**  list_data;      // Pointer to list data (after header)
    int64_t index;          // Current position
    int64_t len;            // List length
    int     ref_count;      // Reference count for COW
} GradientListIter;

/* Range iterator */
typedef struct {
    int     iter_type;      // ITER_TYPE_RANGE
    int64_t current;        // Current value
    int64_t end;            // End value (exclusive)
    int     ref_count;      // Reference count
} GradientRangeIter;

/*
 * list_iter_retain(iter) -> iter
 */
static void* list_iter_retain(GradientListIter* iter) {
    if (iter) iter->ref_count++;
    return iter;
}

/*
 * list_iter_release(iter)
 */
static void list_iter_release(GradientListIter* iter) {
    if (!iter) return;
    iter->ref_count--;
    if (iter->ref_count <= 0) {
        free(iter);
    }
}

/*
 * range_iter_retain(iter) -> iter
 */
static void* range_iter_retain(GradientRangeIter* iter) {
    if (iter) iter->ref_count++;
    return iter;
}

/*
 * range_iter_release(iter)
 */
static void range_iter_release(GradientRangeIter* iter) {
    if (!iter) return;
    iter->ref_count--;
    if (iter->ref_count <= 0) {
        free(iter);
    }
}

/*
 * __gradient_list_iter_new(list) -> Iterator[T]
 *
 * Create a new list iterator. List format: [len, cap, data...]
 */
void* __gradient_list_iter_new(void* list) {
    if (!list) return NULL;

    int64_t* header = (int64_t*)list;
    int64_t len = header[0];

    GradientListIter* iter = (GradientListIter*)malloc(sizeof(GradientListIter));
    iter->iter_type = ITER_TYPE_LIST;
    iter->list_data = (void**)(header + 2);  // Skip len/cap header
    iter->index = 0;
    iter->len = len;
    iter->ref_count = 1;

    return iter;
}

/*
 * __gradient_range_iter_new(start, end) -> Iterator[Int]
 *
 * Create a new range iterator for [start, end).
 */
void* __gradient_range_iter_new(int64_t start, int64_t end) {
    GradientRangeIter* iter = (GradientRangeIter*)malloc(sizeof(GradientRangeIter));
    iter->iter_type = ITER_TYPE_RANGE;
    iter->current = start;
    iter->end = end;
    iter->ref_count = 1;

    return iter;
}

/*
 * __gradient_iter_next_list(iter) -> Option[Ptr]
 *
 * Get next element from list iterator. Returns pointer to value or NULL.
 */
void* __gradient_iter_next_list(void* iter) {
    GradientListIter* it = (GradientListIter*)iter;
    if (!it || it->iter_type != ITER_TYPE_LIST) return NULL;

    if (it->index >= it->len) {
        return NULL;  // No more elements
    }

    // Return pointer to current element and advance
    void* value = it->list_data[it->index];
    it->index++;
    return value;
}

/*
 * __gradient_iter_next_range(iter) -> Option[Int]
 *
 * Get next value from range iterator. Returns boxed int or NULL.
 */
void* __gradient_iter_next_range(void* iter) {
    GradientRangeIter* it = (GradientRangeIter*)iter;
    if (!it || it->iter_type != ITER_TYPE_RANGE) return NULL;

    if (it->current >= it->end) {
        return NULL;  // Range exhausted
    }

    // Box the int and return
    int64_t* result = (int64_t*)malloc(sizeof(int64_t));
    *result = it->current;
    it->current++;
    return result;
}

/*
 * __gradient_iter_has_next_list(iter) -> Int
 *
 * Check if list iterator has more elements (1 = yes, 0 = no).
 */
int64_t __gradient_iter_has_next_list(void* iter) {
    GradientListIter* it = (GradientListIter*)iter;
    if (!it || it->iter_type != ITER_TYPE_LIST) return 0;
    return (it->index < it->len) ? 1 : 0;
}

/*
 * __gradient_iter_has_next_range(iter) -> Int
 *
 * Check if range iterator has more values.
 */
int64_t __gradient_iter_has_next_range(void* iter) {
    GradientRangeIter* it = (GradientRangeIter*)iter;
    if (!it || it->iter_type != ITER_TYPE_RANGE) return 0;
    return (it->current < it->end) ? 1 : 0;
}

/*
 * __gradient_iter_count_list(iter) -> Int
 *
 * Count remaining elements in list iterator (consumes iterator).
 */
int64_t __gradient_iter_count_list(void* iter) {
    GradientListIter* it = (GradientListIter*)iter;
    if (!it || it->iter_type != ITER_TYPE_LIST) return 0;

    int64_t remaining = it->len - it->index;
    it->index = it->len;  // Consume iterator
    return remaining;
}

/*
 * __gradient_iter_count_range(iter) -> Int
 *
 * Count remaining values in range iterator (consumes iterator).
 */
int64_t __gradient_iter_count_range(void* iter) {
    GradientRangeIter* it = (GradientRangeIter*)iter;
    if (!it || it->iter_type != ITER_TYPE_RANGE) return 0;

    int64_t remaining = it->end - it->current;
    it->current = it->end;  // Consume iterator
    return remaining > 0 ? remaining : 0;
}

/*
 * __gradient_iter_retain(iter) -> iter
 *
 * Increment reference count for any iterator type.
 */
void* __gradient_iter_retain(void* iter) {
    if (!iter) return NULL;

    int* type_tag = (int*)iter;
    if (*type_tag == ITER_TYPE_LIST) {
        return list_iter_retain((GradientListIter*)iter);
    } else {
        return range_iter_retain((GradientRangeIter*)iter);
    }
}

/*
 * __gradient_iter_release(iter)
 *
 * Decrement reference count and free iterator.
 */
void __gradient_iter_release(void* iter) {
    if (!iter) return;

    int* type_tag = (int*)iter;
    if (*type_tag == ITER_TYPE_LIST) {
        list_iter_release((GradientListIter*)iter);
    } else {
        range_iter_release((GradientRangeIter*)iter);
    }
}

/*
 * ============================================================================
 * Self-Hosting Phase 1.3: StringBuilder Runtime
 * ============================================================================
 *
 * StringBuilder provides efficient string construction with O(1) amortized
 * append operations. Grows dynamically when capacity is exceeded.
 *
 * Layout:
 *   GradientStringBuilder: { buffer, length, capacity, ref_count }
 */

/* Default initial capacity */
#define SB_DEFAULT_CAPACITY 16

/* StringBuilder structure */
typedef struct {
    char*   buffer;     // Dynamic buffer
    int64_t length;     // Current string length
    int64_t capacity;   // Buffer capacity
    int     ref_count;  // Reference count for COW
} GradientStringBuilder;

/*
 * stringbuilder_retain(sb) -> sb
 */
static void* stringbuilder_retain(GradientStringBuilder* sb) {
    if (sb) sb->ref_count++;
    return sb;
}

/*
 * stringbuilder_release(sb)
 */
static void stringbuilder_release(GradientStringBuilder* sb) {
    if (!sb) return;
    sb->ref_count--;
    if (sb->ref_count <= 0) {
        free(sb->buffer);
        free(sb);
    }
}

/*
 * stringbuilder_grow(sb, min_capacity)
 *
 * Grow buffer to at least min_capacity. Returns 0 on success, -1 on failure.
 */
static int stringbuilder_grow(GradientStringBuilder* sb, int64_t min_capacity) {
    int64_t new_capacity = sb->capacity;
    while (new_capacity < min_capacity) {
        new_capacity *= 2;
    }

    char* new_buffer = (char*)safe_realloc(sb->buffer, new_capacity);
    sb->buffer = new_buffer;
    sb->capacity = new_capacity;
    return 0;
}

/*
 * __gradient_stringbuilder_new() -> StringBuilder
 *
 * Create a new empty StringBuilder with default capacity.
 */
void* __gradient_stringbuilder_new(void) {
    GradientStringBuilder* sb = (GradientStringBuilder*)malloc(sizeof(GradientStringBuilder));
    sb->buffer = (char*)malloc(SB_DEFAULT_CAPACITY);
    sb->buffer[0] = '\0';
    sb->length = 0;
    sb->capacity = SB_DEFAULT_CAPACITY;
    sb->ref_count = 1;
    return sb;
}

/*
 * __gradient_stringbuilder_with_capacity(capacity) -> StringBuilder
 *
 * Create a new StringBuilder with specified initial capacity.
 */
void* __gradient_stringbuilder_with_capacity(int64_t capacity) {
    if (capacity < 1) capacity = SB_DEFAULT_CAPACITY;

    GradientStringBuilder* sb = (GradientStringBuilder*)malloc(sizeof(GradientStringBuilder));
    sb->buffer = (char*)malloc(capacity);
    sb->buffer[0] = '\0';
    sb->length = 0;
    sb->capacity = capacity;
    sb->ref_count = 1;
    return sb;
}

/*
 * __gradient_stringbuilder_append(sb, str) -> StringBuilder
 *
 * Append a string to the builder. Returns the builder (for chaining).
 */
void* __gradient_stringbuilder_append(void* sb, const char* str) {
    GradientStringBuilder* builder = (GradientStringBuilder*)sb;
    if (!builder || !str) return sb;

    int64_t str_len = strlen(str);
    int64_t needed = builder->length + str_len + 1;

    if (needed > builder->capacity) {
        if (stringbuilder_grow(builder, needed) < 0) return sb;
    }

    memcpy(builder->buffer + builder->length, str, str_len + 1);
    builder->length += str_len;
    return sb;
}

/*
 * __gradient_stringbuilder_append_char(sb, c) -> StringBuilder
 *
 * Append a single character (as integer code point).
 */
void* __gradient_stringbuilder_append_char(void* sb, int64_t c) {
    GradientStringBuilder* builder = (GradientStringBuilder*)sb;
    if (!builder) return sb;

    int64_t needed = builder->length + 2;  // char + null terminator

    if (needed > builder->capacity) {
        if (stringbuilder_grow(builder, needed) < 0) return sb;
    }

    builder->buffer[builder->length] = (char)c;
    builder->buffer[builder->length + 1] = '\0';
    builder->length++;
    return sb;
}

/*
 * __gradient_stringbuilder_append_int(sb, n) -> StringBuilder
 *
 * Append an integer as decimal string.
 */
void* __gradient_stringbuilder_append_int(void* sb, int64_t n) {
    GradientStringBuilder* builder = (GradientStringBuilder*)sb;
    if (!builder) return sb;

    // Max int64 is 19 digits + sign + null
    char buf[32];
    snprintf(buf, sizeof(buf), "%ld", n);
    return __gradient_stringbuilder_append(sb, buf);
}

/*
 * __gradient_stringbuilder_length(sb) -> Int
 *
 * Get current string length.
 */
int64_t __gradient_stringbuilder_length(void* sb) {
    GradientStringBuilder* builder = (GradientStringBuilder*)sb;
    if (!builder) return 0;
    return builder->length;
}

/*
 * __gradient_stringbuilder_capacity(sb) -> Int
 *
 * Get current buffer capacity.
 */
int64_t __gradient_stringbuilder_capacity(void* sb) {
    GradientStringBuilder* builder = (GradientStringBuilder*)sb;
    if (!builder) return 0;
    return builder->capacity;
}

/*
 * __gradient_stringbuilder_to_string(sb) -> String
 *
 * Copy builder contents to a new Gradient string and return it.
 */
void* __gradient_stringbuilder_to_string(void* sb) {
    GradientStringBuilder* builder = (GradientStringBuilder*)sb;

    // Create new Gradient string (format: ptr + len)
    void** result = (void**)malloc(2 * sizeof(void*));
    char* str;

    if (!builder || builder->length == 0) {
        // Return empty string
        str = (char*)malloc(1);
        str[0] = '\0';
        result[0] = str;
        result[1] = (void*)0;
    } else {
        str = (char*)malloc(builder->length + 1);
        memcpy(str, builder->buffer, builder->length + 1);
        result[0] = str;
        result[1] = (void*)builder->length;
    }
    return result;
}

/*
 * __gradient_stringbuilder_clear(sb) -> StringBuilder
 *
 * Clear the builder (reset length to 0, keep capacity).
 */
void* __gradient_stringbuilder_clear(void* sb) {
    GradientStringBuilder* builder = (GradientStringBuilder*)sb;
    if (!builder) return sb;

    builder->length = 0;
    builder->buffer[0] = '\0';
    return sb;
}

/*
 * __gradient_stringbuilder_retain(sb) -> sb
 */
void* __gradient_stringbuilder_retain(void* sb) {
    return stringbuilder_retain((GradientStringBuilder*)sb);
}

/*
 * __gradient_stringbuilder_release(sb)
 */
void __gradient_stringbuilder_release(void* sb) {
    stringbuilder_release((GradientStringBuilder*)sb);
}

/*
 * ============================================================================
 * Self-Hosting Phase 1.4: Directory Listing Runtime
 * ============================================================================
 *
 * File system operations for directory listing and file metadata.
 * Used for module discovery in the self-hosting compiler.
 *
 * POSIX implementation using dirent.h
 * Windows implementation would use FindFirstFile/FindNextFile
 */

#include <sys/stat.h>
#include <dirent.h>

/*
 * __gradient_file_list_directory(path) -> List[String]
 *
 * List all entries in a directory. Returns empty list on error or if
 * directory doesn't exist. Includes "." and ".." entries.
 */
void* __gradient_file_list_directory(const char* path) {
    // Create empty list: [len=0, cap=0]
    void* empty_list = malloc(16);
    int64_t* empty_hdr = (int64_t*)empty_list;
    empty_hdr[0] = 0;  /* length */
    empty_hdr[1] = 0;  /* capacity */

    if (!path) {
        return empty_list;
    }

    DIR* dir = opendir(path);
    if (!dir) {
        // Return empty list on error
        return empty_list;
    }

    // Count entries first
    int64_t count = 0;
    struct dirent* entry;
    while ((entry = readdir(dir)) != NULL) {
        count++;
    }
    rewinddir(dir);

    // Allocate list with header [len, cap] + entries
    void* list = malloc((size_t)(16 + count * 8));
    int64_t* hdr = (int64_t*)list;
    hdr[0] = count;   /* length */
    hdr[1] = count;   /* capacity */
    char** data = (char**)(hdr + 2);

    // Fill entries
    int64_t i = 0;
    while ((entry = readdir(dir)) != NULL && i < count) {
        data[i] = strdup(entry->d_name);
        i++;
    }

    closedir(dir);
    free(empty_list);  // Free the temporary empty list
    return list;
}

/*
 * __gradient_file_is_directory(path) -> Int
 *
 * Returns 1 if path is a directory, 0 otherwise (including errors).
 */
int64_t __gradient_file_is_directory(const char* path) {
    if (!path) return 0;

    struct stat st;
    if (stat(path, &st) != 0) {
        return 0;
    }

    return S_ISDIR(st.st_mode) ? 1 : 0;
}

/*
 * __gradient_file_size(path) -> Option[Int]
 *
 * Returns Some(size) if file exists, None otherwise.
 * For directories, behavior is platform-specific.
 */
void* __gradient_file_size(const char* path) {
    if (!path) {
        // Return None
        void** result = (void**)malloc(2 * sizeof(void*));
        result[0] = (void*)0;  // None discriminant
        result[1] = NULL;
        return result;
    }

    struct stat st;
    if (stat(path, &st) != 0) {
        // Return None
        void** result = (void**)malloc(2 * sizeof(void*));
        result[0] = (void*)0;  // None discriminant
        result[1] = NULL;
        return result;
    }

    // Return Some(size)
    void** result = (void**)malloc(2 * sizeof(void*));
    result[0] = (void*)1;  // Some discriminant
    int64_t* size = (int64_t*)malloc(sizeof(int64_t));
    *size = st.st_size;
    result[1] = size;
    return result;
}

/* ── Phase 0: String Primitives for Self-Hosting ─────────────────────────
 *
 * These functions provide low-level string operations required for the
 * self-hosted Gradient compiler. They enable the lexer to scan source code
 * character-by-character.
 *
 *   string_length(s)      -> Returns length of string in bytes
 *   string_char_at(s, idx)  -> Returns byte at index (as int64_t), or -1 if out of bounds
 *   string_substring(s, start, end) -> Returns substring [start, end)
 *   string_append(a, b)     -> Returns concatenated string (a + b)
 */

/*
 * __gradient_string_length(s: const char*) -> int64_t
 *
 * Returns the length of the string in bytes (not counting null terminator).
 * Returns 0 if s is NULL.
 */
int64_t __gradient_string_length(const char* s) {
    if (!s) return 0;
    return (int64_t)strlen(s);
}

/*
 * __gradient_string_char_at(s: const char*, idx: int64_t) -> char*
 *
 * Returns a heap-allocated single-character string at the given index.
 * Returns empty string if index is out of bounds or if s is NULL.
 * Caller owns the returned string.
 */
char* __gradient_string_char_at(const char* s, int64_t idx) {
    if (!s) return strdup("");
    if (idx < 0) return strdup("");
    size_t len = strlen(s);
    if ((size_t)idx >= len) return strdup("");
    char* result = (char*)malloc(2);
    if (!result) return strdup("");
    result[0] = s[idx];
    result[1] = '\0';
    return result;
}

/*
 * __gradient_string_char_code_at(s: const char*, idx: int64_t) -> int64_t
 *
 * Returns the byte at the given index as an int64_t.
 * Returns -1 if the index is out of bounds or if s is NULL.
 * This is the primitive needed for self-hosted lexer.
 * Note: This returns bytes, not Unicode codepoints. For ASCII source
 * code (which Gradient is), this is sufficient.
 */
int64_t __gradient_string_char_code_at(const char* s, int64_t idx) {
    if (!s) return -1;
    if (idx < 0) return -1;
    size_t len = strlen(s);
    if ((size_t)idx >= len) return -1;
    return (int64_t)(unsigned char)s[idx];
}

/*
 * __gradient_string_substring(s: const char*, start: int64_t, end: int64_t) -> char*
 *
 * Returns a heap-allocated substring from [start, end).
 * Returns empty string on invalid parameters.
 * Caller owns the returned string.
 */
char* __gradient_string_substring(const char* s, int64_t start, int64_t end) {
    if (!s) return strdup("");
    if (start < 0) start = 0;
    if (end < start) end = start;
    size_t len = strlen(s);
    if ((size_t)start >= len) return strdup("");
    if ((size_t)end > len) end = (int64_t)len;
    size_t sublen = (size_t)(end - start);
    char* result = (char*)malloc(sublen + 1);
    if (!result) return strdup("");
    memcpy(result, s + start, sublen);
    result[sublen] = '\0';
    return result;
}

/*
 * __gradient_string_append(a: const char*, b: const char*) -> char*
 *
 * Returns a heap-allocated string containing a followed by b.
 * Returns empty string if both are NULL.
 * Caller owns the returned string.
 */
char* __gradient_string_append(const char* a, const char* b) {
    if (!a && !b) return strdup("");
    if (!a) return strdup(b);
    if (!b) return strdup(a);
    size_t len_a = strlen(a);
    size_t len_b = strlen(b);
    char* result = (char*)malloc(len_a + len_b + 1);
    if (!result) return strdup("");
    memcpy(result, a, len_a);
    memcpy(result + len_a, b, len_b);
    result[len_a + len_b] = '\0';
    return result;
}