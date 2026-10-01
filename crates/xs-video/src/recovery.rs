//! Once an H.264 reference frame is lost, delta frames must not reach the decoder
//! until a successfully delivered keyframe repairs its reference chain.
#[derive(Default)]
pub(crate) struct FrameRecovery {
    waiting: bool,
}

impl FrameRecovery {
    pub fn accepts(&self, keyframe: bool) -> bool {
        !self.waiting || keyframe
    }
    pub fn dropped(&mut self) {
        self.waiting = true;
    }
    pub fn delivered(&mut self, keyframe: bool) {
        if keyframe {
            self.waiting = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_a_delivered_keyframe_repairs_a_broken_reference_chain() {
        let mut recovery = FrameRecovery::default();
        assert!(recovery.accepts(false));
        recovery.dropped();
        assert!(!recovery.accepts(false));
        assert!(recovery.accepts(true));
        recovery.dropped(); // Even a keyframe can be lost to a full queue.
        assert!(!recovery.accepts(false));
        recovery.delivered(false);
        assert!(!recovery.accepts(false));
        recovery.delivered(true);
        assert!(recovery.accepts(false));
    }
}
