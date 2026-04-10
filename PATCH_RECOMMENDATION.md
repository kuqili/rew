# Recommended Patch: Sync Hook Temp File Filtering

## Problem
The hook's `post-tool` handler records temp files that the daemon's file watcher would filter. This creates inconsistency in the changes table.

## Root Cause
Hook's `is_temp_file()` function (hook.rs:437) only checks for:
- `.tmp.*` (Claude Code Write staging)
- `.swp`, `.swo` (Vim)
- `~` (Backup suffix)

Daemon's `is_temp_path()` function also filters:
- `.sb-*` (macOS safe-save atomic writes)
- `.temp` (Generic temp extension)
- `.#*` (Emacs lock files)

## Solution
Expand hook's `is_temp_file()` to match daemon's patterns.

## File to Modify
`crates/rew-cli/src/commands/hook.rs`, lines 437-453

## Current Code
```rust
/// Check if a path looks like a temporary file created by AI tools.
///
/// Claude Code's Write tool creates `.tmp.XXXXX.XXXXX` staging files;
/// editors may create `.swp`, `~` backup files, etc. These should be
/// silently ignored by hooks.
fn is_temp_file(path: &Path) -> bool {
    let name = match path.file_name().and_then(|n| n.to_str()) {
        Some(n) => n,
        None => return false,
    };

    // Claude Code Write tool: "foo.rs.tmp.73919.177..."
    if name.contains(".tmp.") {
        return true;
    }
    // Editor swap / backup files
    if name.ends_with(".swp") || name.ends_with(".swo") || name.ends_with('~') {
        return true;
    }

    false
}
```

## Updated Code
```rust
/// Check if a path looks like a temporary file created by AI tools or editors.
///
/// Claude Code's Write tool creates `.tmp.XXXXX.XXXXX` staging files;
/// macOS apps create `.sb-XXXXXXXX-YYYYYY` for atomic writes;
/// editors create `.swp`, `~`, `.#*` files, etc.
/// These should be silently ignored by hooks.
fn is_temp_file(path: &Path) -> bool {
    let name = match path.file_name().and_then(|n| n.to_str()) {
        Some(n) => n,
        None => return false,
    };

    // Claude Code Write tool: "foo.rs.tmp.73919.177..."
    if name.contains(".tmp.") {
        return true;
    }

    // macOS safe-save (atomic write): "original.sb-XXXXXXXX-YYYYYY"
    // The ".sb-" marker can appear anywhere after the real filename.
    if name.contains(".sb-") {
        return true;
    }

    // Generic temp extensions
    if name.ends_with(".temp") {
        return true;
    }

    // Emacs lock files: ".#symlink"
    if name.starts_with(".#") {
        return true;
    }

    // Editor swap / backup files
    if name.ends_with(".swp") || name.ends_with(".swo") || name.ends_with('~') {
        return true;
    }

    false
}
```

## Changes Summary

| What | Before | After | Change |
|------|--------|-------|--------|
| Lines | 437-453 | 437-465 | +12 |
| Patterns checked | 4 | 7 | +3 |
| Comments | 2 | 5 | +3 |
| Code lines | 9 | 17 | +8 |

## Patterns Added

1. **`.sb-*`** (macOS safe-save)
   - Used by apps for atomic file writes
   - Pattern: `filename.sb-XXXXXXXX-YYYYYY`
   - Example: `config.json.sb-12345678-abcdef`

2. **`.temp`** (Generic temp extension)
   - Common temp file suffix
   - Example: `temp_config.temp`

3. **`.#*`** (Emacs lock files)
   - Emacs creates these as lock files
   - Pattern: `.#filename`
   - Example: `.#main.rs`

## Impact

### Before Patch
```
macOS app saves file atomically:
  1. Writes to config.sb-12345678
  2. Hook's is_temp_file() returns false
  3. Changes recorded to DB ❌ (incorrect)
  4. Daemon receives event, filters via is_temp_path()
  5. Daemon ignores it (correct)
  
Result: DB has spurious temp file record
```

### After Patch
```
macOS app saves file atomically:
  1. Writes to config.sb-12345678
  2. Hook's is_temp_file() returns true
  3. Changes NOT recorded ✓ (correct)
  4. Daemon receives event, filters via is_temp_path()
  5. Daemon ignores it (correct)
  
Result: DB stays clean
```

## Risk Assessment

**Very Low Risk**

- **Scope**: Only affects pre-DB filtering in hook path
- **Type**: Additive (new patterns, no removal of existing patterns)
- **Backward compat**: 100% compatible (only filters more, never less)
- **Performance**: Negligible (string checks are O(1))
- **Testing**: Can be tested locally with test files

## Testing Procedure

1. Create test files with temp patterns:
   ```bash
   touch /path/to/watched/file.sb-12345678
   touch /path/to/watched/config.temp
   touch /path/to/watched/.#tempfile
   ```

2. Trigger Claude Code Write operations on these files

3. Check changes table:
   ```sql
   SELECT * FROM changes WHERE file_path LIKE '%sb-%' OR file_path LIKE '%.temp' OR file_path LIKE '%#%';
   ```

4. Verify NO records appear (or expected records appear if they came from daemon path)

## Deployment

### Before Deployment
- [ ] Run existing unit tests
- [ ] Local testing with temp files
- [ ] Code review

### After Deployment
- [ ] Monitor for any regressions
- [ ] Verify spurious temp files no longer appear in changes table
- [ ] Consider backfilling DB to remove existing spurious records (if needed)

## Related Files

- **Daemon filter**: `src-tauri/src/daemon.rs:795` (reference implementation)
- **Core filter**: `crates/rew-core/src/watcher/filter.rs:40` (glob patterns)
- **Hook pre-tool**: `crates/rew-cli/src/commands/hook.rs:268` (also checks is_temp_file)

## Notes

- The hook's `handle_pre_tool()` function (line 268) also calls `is_temp_file()`, so this patch benefits both post-tool and pre-tool paths
- This aligns the hook's filtering with the daemon's filtering, creating consistency across the system
- The patterns chosen match those used by daemon's `is_temp_path()` function, ensuring unified filtering rules

