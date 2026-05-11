Phase 1 stale-exe boundary split: complete.
- Running drive PIDs with deleted/replaced executable inodes are now advisory-only in auto-drive watchdog.
- No mark_drive_failed, stale_binary_inode blocked_reason, or silent_zombie lock close is written solely for alive post-spawn stale exe drift.
- Pre-spawn daemon stale reexec/refuse code in agents_run.rs was not changed.
- Targeted tests passed: watchdog_alive_pid_with_deleted_exe_is_advisory_no_block; daemon_stale_messages_keep_reexec_and_fail_loud_boundaries.
