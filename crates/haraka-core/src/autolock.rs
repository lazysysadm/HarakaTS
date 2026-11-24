//! Politique de verrouillage automatique par inactivité (logique pure, testable).
//! Les signaux système (veille, verrouillage de session) sont câblés dans
//! src-tauri (zbus) et appellent `Vault::lock`.

use std::time::{Duration, Instant};

#[derive(Debug)]
pub struct AutoLock {
    delay: Option<Duration>,
    last_activity: Instant,
}

impl AutoLock {
    /// `minutes == 0` → verrouillage par inactivité désactivé.
    pub fn new(minutes: u32) -> Self {
        Self {
            delay: if minutes == 0 {
                None
            } else {
                Some(Duration::from_secs(u64::from(minutes) * 60))
            },
            last_activity: Instant::now(),
        }
    }

    pub fn with_delay(delay: Duration) -> Self {
        Self {
            delay: Some(delay),
            last_activity: Instant::now(),
        }
    }

    pub fn activity(&mut self) {
        self.last_activity = Instant::now();
    }

    pub fn set_delay_minutes(&mut self, minutes: u32) {
        self.delay = if minutes == 0 {
            None
        } else {
            Some(Duration::from_secs(u64::from(minutes) * 60))
        };
        self.last_activity = Instant::now();
    }

    pub fn delay_minutes(&self) -> u32 {
        self.delay.map(|d| (d.as_secs() / 60) as u32).unwrap_or(0)
    }

    pub fn should_lock(&self) -> bool {
        match self.delay {
            Some(d) => self.last_activity.elapsed() >= d,
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_never_locks() {
        let lock = AutoLock::new(0);
        assert!(!lock.should_lock());
    }

    #[test]
    fn locks_after_delay_and_activity_resets() {
        let mut lock = AutoLock::with_delay(Duration::from_millis(20));
        assert!(!lock.should_lock());
        std::thread::sleep(Duration::from_millis(30));
        assert!(lock.should_lock());
        lock.activity();
        assert!(!lock.should_lock());
    }
}
