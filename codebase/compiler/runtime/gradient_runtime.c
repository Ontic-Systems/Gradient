/*
 * Gradient Runtime Helpers — Phase MM: Standard I/O
 *
 * Compile and link with your compiled Gradient object:
 *   cc gradient_runtime.c -c -o gradient_runtime.o
 *   cc your_program.o gradient_runtime.o -o your_program
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

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
