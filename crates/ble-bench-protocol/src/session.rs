//! Reject recorded sessions without resetting active delivery state.
//! Fresh challenges come from the platform CSPRNG. This is not authentication.
#[derive(Default)]
pub struct Session {
    pub active: u128,
    pub budget: u32,
    client: u128,
    pending: Option<(u128, u128, u32)>,
}
#[derive(Debug, PartialEq, Eq)]
pub enum Confirm {
    New,
    Duplicate,
    Reject,
}
impl Session {
    pub fn challenge(&mut self, client: u128, budget: u32, fresh: u128) -> Option<u128> {
        if client == 0 || fresh == 0 || !(64..=8192).contains(&budget) {
            return None;
        }
        if self.active != 0 && client == self.client {
            return (budget == self.budget).then_some(self.active);
        }
        if let Some((c, session, b)) = self.pending {
            if c == client {
                return (budget == b).then_some(session);
            }
        }
        if fresh == self.active {
            return None;
        }
        self.pending = Some((client, fresh, budget));
        Some(fresh)
    }
    pub fn confirm(&mut self, client: u128, session: u128) -> Confirm {
        if session != 0 && session == self.active && client == self.client {
            return Confirm::Duplicate;
        }
        if let Some((c, s, budget)) = self.pending {
            if client == c && session == s {
                self.active = s;
                self.client = c;
                self.budget = budget;
                self.pending = None;
                return Confirm::New;
            }
        }
        Confirm::Reject
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn replay_cannot_reopen_a_retired_session_even_after_reboot() {
        let mut s = Session::default();
        assert_eq!(s.challenge(11, 1024, 101), Some(101));
        assert_eq!(s.confirm(11, 101), Confirm::New);
        assert_eq!(s.confirm(11, 101), Confirm::Duplicate);
        assert_eq!(s.challenge(22, 1024, 202), Some(202));
        assert_eq!(s.active, 101); // HELLO does not reset the live session.
        assert_eq!(s.confirm(22, 202), Confirm::New);
        assert_eq!(s.challenge(11, 1024, 303), Some(303));
        assert_eq!(s.confirm(11, 101), Confirm::Reject);
        assert_eq!(s.active, 202);
        let mut rebooted = Session::default();
        assert_eq!(rebooted.challenge(11, 1024, 404), Some(404));
        assert_eq!(rebooted.confirm(11, 101), Confirm::Reject);
    }
}
