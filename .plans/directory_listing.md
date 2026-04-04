# Phase 1.4 Implementation Plan: Directory Listing

## Goal
Implement file system directory listing capabilities for the Gradient self-hosting compiler. This enables module discovery by listing files in directories.

## Requirements

### Functional Requirements
1. **List directory contents**: Get all files/directories in a path
2. **Check if path is directory**: Distinguish files from directories
3. **Check if path exists**: Verify file/directory existence
4. **Cross-platform**: Work on Linux, macOS, Windows
5. **Error handling**: Graceful handling of permission errors, non-existent paths

### API Design

```gradient
// List all entries in a directory
fn file_list_directory(path: String) -> !{IO} List[String]

// Check if path is a directory
fn file_is_directory(path: String) -> !{IO} Bool

// Check if path exists (file or directory)
fn file_exists(path: String) -> !{IO} Bool

// Get file size in bytes
fn file_size(path: String) -> !{IO} Option[Int]

// Usage example for module discovery:
fn find_gradient_modules(dir: String) -> !{IO} List[String]:
    let entries = file_list_directory(dir)
    let modules = list_filter(entries, fn(entry) ->
        string_ends_with(entry, ".gradient")
    )
    ret modules
```

## Implementation Strategy

### Phase A: Type System and Builtin Functions

Add to `env.rs`:
- `file_list_directory(path: String) -> List[String]` (IO effect)
- `file_is_directory(path: String) -> Bool` (IO effect)
- `file_exists(path: String) -> Bool` (IO effect)
- `file_size(path: String) -> Option[Int]` (IO effect)

### Phase B: C Runtime Implementation

Implement in `gradient_runtime.c`:

```c
// List directory entries
char** __gradient_file_list_directory(const char* path, int64_t* count);

// Check if directory
int64_t __gradient_file_is_directory(const char* path);

// Check if exists
int64_t __gradient_file_exists(const char* path);

// Get file size
void* __gradient_file_size(const char* path);  // Returns Option[Int]
```

Implementation details:
- Use `dirent.h` for POSIX systems (Linux, macOS)
- Use `FindFirstFile`/`FindNextFile` for Windows
- Return empty list for non-existent directories
- Return properly formatted Gradient strings

### Phase C: Tests

Test scenarios:
1. List existing directory
2. List non-existent directory (returns empty list)
3. Check is_directory on file vs directory
4. Check exists on existing vs non-existent paths
5. Get file size on existing file vs non-existent

## File Structure Changes

```
codebase/compiler/src/
├── typechecker/
│   └── env.rs            # Add 4 file builtins
└── codegen/
    └── ...               # Add runtime calls

runtime/
└── gradient_runtime.c    # Add directory operations
```

## Implementation Order

1. **Builtin functions** (20 min)
   - Add to env.rs

2. **C runtime** (1 hour)
   - POSIX implementation using dirent.h
   - Windows stub (can be filled in later)
   - String conversion helpers

3. **Tests** (30 min)
   - Unit tests for each function
   - Integration test with temp directory

## Testing Plan

```gradient
// Test 1: List directory
fn test_list_directory():
    let entries = file_list_directory("/tmp")
    assert(list_len(entries) > 0)

// Test 2: Check directory
fn test_is_directory():
    let is_dir = file_is_directory("/tmp")
    assert(is_dir == true)
    let is_file = file_is_directory("/etc/passwd")
    assert(is_file == false)

// Test 3: Check exists
fn test_exists():
    let exists = file_exists("/tmp")
    assert(exists == true)
    let not_exists = file_exists("/nonexistent_path_xyz")
    assert(not_exists == false)
```

## Risks and Mitigations

| Risk | Mitigation |
|------|------------|
| Platform differences | Separate implementations for POSIX/Windows |
| Permission errors | Return empty list, don't crash |
| Memory management | Proper allocation/deallocation of strings |
| Symlinks | Follow symlinks by default (standard behavior) |

## Definition of Done

- [ ] `file_list_directory` works on Linux/macOS
- [ ] `file_is_directory` returns correct results
- [ ] `file_exists` returns correct results
- [ ] `file_size` returns correct results
- [ ] All tests passing
- [ ] No regressions in existing tests
- [ ] Documentation updated

## Timeline Estimate

- Builtin functions: 20 min
- C runtime (POSIX): 1 hour
- Tests: 30 min

**Total: ~2 hours**
