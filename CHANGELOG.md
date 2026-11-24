# Changelog

Versions follow SemVer. The detailed history is in the table at the end of [MODE_D_EMPLOI.md](MODE_D_EMPLOI.md) (French) and [MODE_D_EMPLOI.en.md](MODE_D_EMPLOI.en.md).

## 1.0.9
- Settings → Known hosts: new "Clear all" button. It empties only HarakaTS's own file and keeps a `known_hosts.BAK` backup. `~/.ssh/known_hosts` is never touched.

## 1.0.8
- Removed the latency indicator (its ping could freeze sessions on non-OpenSSH servers).

## 1.0.7
- Fixed a ~30 s freeze of every session caused by a global lock held during a network call.

## 1.0.6
- Toast feedback after save/validate buttons.

## 1.0.5
- Fixed a reconnect deadlock (empty terminal after closing and reopening a session).

## 1.0.4
- Clearer authentication errors, with the methods tried and the methods the server offers.
- Automatic password prompt when the configured methods fail.
- The terminal-following hook is injected only on OpenSSH servers, so MikroTik, Cisco and similar CLIs are left alone.

## 1.0.0 – 1.0.3
- First stable release: encrypted portable vault, SSH/SFTP, native VTE terminal in tabs, tunnels, macros, multi-exec, remote editing, six UI languages, `.deb` and `.rpm` packages.
- 1.0.2: security hardening (known_hosts fail-closed, lighter dependency set).
