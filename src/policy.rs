// This dependency-free module is used verbatim by the CLI and by the proof harnesses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Redirect {
    DropBody,
    PreserveBody,
}

impl Redirect {
    pub fn from_status(status: u16) -> Option<Self> {
        match status {
            301..=303 => Some(Self::DropBody),
            307 | 308 => Some(Self::PreserveBody),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Denied {
    Limit,
    Downgrade,
    CrossOriginWrite,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Chain {
    https: bool,
    user_headers: bool,
    body: bool,
    remaining: u32,
}

impl Chain {
    pub fn start(https: bool, body: bool, remaining: u32) -> Self {
        Self {
            https,
            user_headers: true,
            body,
            remaining,
        }
    }

    pub fn user_headers(self) -> bool {
        self.user_headers
    }
    pub fn body(self) -> bool {
        self.body
    }

    pub fn exhausted(self) -> bool {
        self.remaining == 0
    }

    pub fn follow(
        self,
        redirect: Redirect,
        same_origin: bool,
        target_https: bool,
        explicit_method_other_than_get_head: bool,
    ) -> Result<Self, Denied> {
        let remaining = self.remaining.checked_sub(1).ok_or(Denied::Limit)?;
        if self.https && !target_https {
            return Err(Denied::Downgrade);
        }
        let body = self.body && redirect == Redirect::PreserveBody;
        if !same_origin && (body || explicit_method_other_than_get_head) {
            return Err(Denied::CrossOriginWrite);
        }
        Ok(Self {
            https: target_https,
            user_headers: self.user_headers && same_origin,
            body,
            remaining,
        })
    }
}

// Charge the raw input before stripping bytes, including any inserted separator.
pub fn charge(used: usize, additional: usize, limit: usize) -> Option<usize> {
    used.checked_add(additional).filter(|&total| total <= limit)
}

// Count only decoded transfer-body bytes offered to the output writer.
pub fn take_prefix(remaining: &mut u64, offered: usize) -> usize {
    let accepted = (*remaining).min(offered as u64);
    *remaining -= accepted;
    accepted as usize
}
