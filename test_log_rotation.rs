// This demonstrates how tracing_appender::rolling::daily works
// According to the tracing-appender 0.2 documentation:
// - rolling::daily(dir, prefix) creates files named: prefix.YYYY-MM-DD
// - It creates a NEW file each day at the rotation boundary
// - The appender always writes to ONE active file
// - When the day changes (at UTC midnight or configured boundary),
//   it closes the old file and opens a new one with the new date

// Critical insight: The appender tracks the CURRENT file internally.
// It does NOT use mtime to determine which file to write to.
// It uses the date to determine the filename, and keeps the handle open.

// The bug report claims:
// "Both the newly rotated archive and freshly created active file will 
//  have recently-touched mtimes"
// 
// However: the old file is NOT touched after rotation. When rotation happens:
// 1. Appender closes the OLD file (e.g., aperio.log.2026-06-14)
// 2. Appender opens a NEW file (e.g., aperio.log.2026-06-15)
// 3. Old file mtime = when it was last written to (end of day 14)
// 4. New file mtime = when it was created (start of day 15)
//
// The NEW file will have a newer mtime than the OLD file.
// There is NO race where they have "nearly identical mtimes".

// Edge case analysis:
// 1. Clock adjustment: If system clock jumps backward, the appender
//    might try to write to a file dated in the future. But the appender
//    itself handles date comparison, not mtime. Files would still have
//    distinct mtimes based on when they were actually touched.
//
// 2. Multiple rotations in one second: The appender creates one file
//    per day based on the date. Multiple rotations per second is not
//    possible unless the system clock jumps by days, which would require
//    a massive leap. Even then, each file's mtime would reflect when
//    it was created/written to.
//
// 3. newest_log_file() is used for UI display and clear() operation:
//    - recent_lines(): displays the tail of the newest log
//    - clear(): keeps the newest file, removes older ones
//    
// For clear(), the goal is to skip deleting the file being actively
// written to. The appender has that file open and will fail the
// remove_file() call on Windows anyway. On Unix, the file would be
// orphaned but the inode would be kept alive until the appender closes it.
// Using mtime to identify it is a reasonable heuristic.
