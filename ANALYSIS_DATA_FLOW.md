# REW Data Flow Analysis: Temp File Filtering

## Question
The daemon's file watcher already filters temp files (like `.tmp.XXXXX`), but temp files still appear in the changes table. Are these two independent paths? Does the temp file show up because the hook path doesn't go through the daemon's filter?

## Answer: YES — Two Independent Paths, Hook Skips Daemon Filter

The hook's `post-tool` handler **bypasses** the daemon's file watcher filter. Both are independent paths into the database.

---

## Data Flow Diagram

```
┌─────────────────────────────────────────────────────────────────┐
│                    AI TOOL (Claude Code)                        │
└────────────────────────┬────────────────────────────────────────┘
                         │
        ┌────────────────┼────────────────┐
        │                │                │
   (hook prompt)     (pre-tool)      (post-tool)
        │                │                │
        ↓                ↓                ↓
   ┌─────────────┐ ┌──────────────┐ ┌─────────────────────────────┐
   │ Create Task │ │ Scope Check  │ │ POST-TOOL HANDLER           │
   │ in DB       │ │ Backup Orig  │ │ (hook.rs:317)               │
   └─────────────┘ └──────────────┘ │                             │
                                      │ 1. Read stdin JSON          │
        ┌──────────────────────────────┤ 2. Extract file_path       │
        │                              │ 3. ❌ is_temp_file() check │
        │ .current_task marker         │ 4. Record in DB            │
        │ (daemon reads this)           │ 5. Store object            │
        │                              └─────────────────────────────┘
        │                                    ↓
        ↓                            ┌──────────────────────┐
   ┌────────────────┐               │  changes table       │
   │ FILE WATCHER   │               │  ────────────────    │
   │ (daemon)       │               │  task_id: t0101...   │
   │ ────────────   │               │  file_path: foo.rs   │
   │ Receives FSEvent               │  change_type: Modified
   │ ├─ Created     │               │  old_hash: abc...    │
   │ ├─ Modified    │               │  new_hash: def...    │
   │ └─ Deleted     │               └──────────────────────┘
   └────────────────┘
        │
        ↓
   ┌───────────────────────────────┐
   │ PIPELINE (processor)          │
   │ • Deduplication               │
   │ • Aggregation (30s windows)   │
   │ • Dynamic pause filters       │
   └───────────────────────────────┘
        │
        ↓
   ┌────────────────────────────────┐
   │ DAEMON EVENT HANDLER           │
   │ (daemon.rs:382)                │
   │ 1. Filter temp via is_temp_path│
   │ 2. Filter suppressed_paths     │
   │ 3. Filter dir_ignore config    │
   │ 4. Record in DB if not filtered│
   └────────────────────────────────┘
        │
        ↓
   ┌──────────────────────┐
   │  changes table       │
   │  ────────────────    │
   │  (only if passed     │
   │   all 3 filters)     │
   └──────────────────────┘
```

---

## Two Independent Paths

### PATH 1: Hook Post-Tool (Direct, Synchronous)
**File: `crates/rew-cli/src/commands/hook.rs:317`**

```rust
pub fn handle_post_tool() -> RewResult<()> {
    let raw = read_stdin_text();
    let input = match normalize_post_tool(&raw) {
        Some(v) => v,
        None => return Ok(()),  // Can't parse → skip silently
    };
    
    let rew_dir = rew_home_dir();
    let task_id = read_current_task_id(&rew_dir);
    
    if let (Some(task_id), Some(ref path_str)) = (task_id, input.file_path) {
        let path = canonicalize_path(path_str);
        
        // ✅ TEMP FILE CHECK (hook.rs:330)
        if is_temp_file(&path) {
            return Ok(());  // Skip silently
        }
        
        let db = open_db()?;
        
        // Determine change type, get hashes, record change
        let change = Change {
            id: None,
            task_id,
            file_path: path,
            change_type,
            old_hash,
            new_hash,
            diff_text: None,
            lines_added: 0,
            lines_removed: 0,
            restored_at: None,
        };
        
        db.upsert_change(&change)?;  // <-- Records directly to DB
    }
    
    Ok(())
}
```

**Key characteristics:**
- Receives JSON from stdin (Claude Code `PostToolUse` event)
- Reads `.current_task` marker to know which task to attribute the change to
- **Has temp file filtering** (line 330-332):
  ```rust
  fn is_temp_file(path: &Path) -> bool {
      let name = match path.file_name().and_then(|n| n.to_str()) {
          Some(n) => n,
          None => return false,
      };
      if name.contains(".tmp.") {  // Claude Code Write staging files
          return true;
      }
      if name.ends_with(".swp") || name.ends_with(".swo") || name.ends_with('~') {
          return true;
      }
      false
  }
  ```
- **Records directly to `changes` table** via `db.upsert_change(&change)` — **bypasses daemon pipeline entirely**

### PATH 2: Daemon File Watcher (Async, Batch-Oriented)
**File: `src-tauri/src/daemon.rs:382`**

The daemon receives FSEvents through a pipeline, applies 3 layers of filtering, and only then records to the DB.

**Key characteristics:**
- Receives `FileEvent`s from FSEvents watcher (async, batch-driven)
- **Has 3 layers of filtering**:
  1. **Temp file filter** (`is_temp_path()` at line 383)
  2. **Suppressed paths** (GUI operations, TTL 60s)
  3. **Dir-level ignore config** (per-directory exclusions)
- Only records to DB **if ALL filters pass**

---

## Filtering Layer Comparison

| Layer | Hook (post-tool) | Daemon (watcher) | Coverage |
|-------|------------------|------------------|----------|
| **Temp files** | ✅ `.tmp.` only | ✅ `.sb-`, `.tmp`, `.temp`, `.#` | **Hook is narrower** |
| **Suppressed paths** | ❌ None | ✅ 60s TTL | **Daemon only** |
| **Dir ignore config** | ❌ None | ✅ Per-dir rules | **Daemon only** |

---

## Why Temp Files Still Appear

**The hook's `is_temp_file()` (hook.rs:437) only checks for `.tmp.` (Claude Code staging files) and editor backup extensions.**

But the daemon's `is_temp_path()` (daemon.rs:795) also filters:
- `.sb-` (macOS safe-save atomic writes)
- `.temp` (generic temp extension)
- `.#` (Emacs lock files)

### Scenario: If Claude Code creates `.sb-XXXXXXXX` files

1. **Hook path**: File is NOT in the `.tmp.` pattern → `is_temp_file()` returns `false` → **recorded**
2. **Daemon path**: File ends with `.sb-` → `is_temp_path()` returns `true` → **filtered out**

**Result**: If the hook processes the file (posts the change), it gets recorded despite being temp.

---

## The .current_task Coordination

Both paths use the same task ID marker but independently:

```
Hook:
  1. hook prompt → creates .current_task marker
  2. post-tool → reads .current_task → records change
  
Daemon:
  1. Reads .current_task marker (daemon.rs:439: read_active_ai_task_id())
  2. If active AI task → attribute all FSEvents to that task_id
  3. If no AI task → open monitoring window (fs_MMDDHHMMSS)
```

**The marker is the only communication**. They don't coordinate filtering rules.

---

## Solutions

### Option 1: Sync Hook Temp Patterns to Daemon (RECOMMENDED)
**In `hook.rs:437`, expand `is_temp_file()` to match daemon patterns:**

```rust
fn is_temp_file(path: &Path) -> bool {
    let name = match path.file_name().and_then(|n| n.to_str()) {
        Some(n) => n,
        None => return false,
    };
    
    // Claude Code Write tool
    if name.contains(".tmp.") {
        return true;
    }
    
    // macOS safe-save (atomic write)
    if name.contains(".sb-") {
        return true;
    }
    
    // Generic temp extensions
    if name.ends_with(".temp") {
        return true;
    }
    
    // Emacs lock files
    if name.starts_with(".#") {
        return true;
    }
    
    // Editor backup files
    if name.ends_with(".swp") || name.ends_with(".swo") || name.ends_with('~') {
        return true;
    }
    
    false
}
```

### Option 2: Extract Shared Filter
Create a shared filter module that both paths use:

```rust
// crates/rew-core/src/filter.rs
pub fn is_temp_file(path: &Path) -> bool {
    // Shared implementation
}
```

Then both call it:
- `hook.rs`: `use rew_core::filter::is_temp_file;`
- `daemon.rs`: Use `is_temp_file()` instead of `is_temp_path()`

### Option 3: Daemon Post-Processing
The daemon could provide a "curated" change list that filters records created by the hook that match temp patterns (lower-impact, but reactive).

---

## Root Cause Summary

✅ **Confirmed**: Hook and daemon are **independent entry points** to the changes table
✅ **Confirmed**: Hook skips the daemon's full filtering pipeline
⚠️  **Issue**: Hook's temp filter is **narrower** than daemon's — only checks `.tmp.` and `.swp`, not `.sb-` or `.temp`

**The fix**: Expand hook's `is_temp_file()` to match daemon's patterns, or extract to a shared module.
