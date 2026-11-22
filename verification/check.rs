// Standalone harness: Kani compiles the same functions that ship in the CLI.
mod policy {
    include!("../src/policy.rs");

    #[kani::proof]
    fn classify_every_http_status() {
        let status: u16 = kani::any();
        let mode = Redirect::from_status(status);
        assert_eq!(
            mode == Some(Redirect::DropBody),
            status == 301 || status == 302 || status == 303
        );
        assert_eq!(
            mode == Some(Redirect::PreserveBody),
            status == 307 || status == 308
        );
    }

    #[kani::proof]
    fn initial_request() {
        let https = kani::any();
        let body = kani::any();
        let limit = kani::any();
        let state = Chain::start(https, body, limit);
        assert!(state.user_headers());
        assert_eq!(state.https, https);
        assert_eq!(state.body(), body);
        assert_eq!(state.remaining, limit);
    }

    fn arbitrary_state() -> Chain {
        Chain {
            https: kani::any(),
            user_headers: kani::any(),
            body: kani::any(),
            remaining: kani::any(),
        }
    }

    #[kani::proof]
    fn every_accepted_step_preserves_invariants() {
        let prev = arbitrary_state();
        let same_origin = kani::any();
        let https = kani::any();
        let other_method: bool = kani::any();
        let preserve: bool = kani::any();
        let mode = if preserve {
            Redirect::PreserveBody
        } else {
            Redirect::DropBody
        };
        if let Ok(next) = prev.follow(mode, same_origin, https, other_method) {
            assert!(prev.remaining > 0);
            assert_eq!(u64::from(next.remaining) + 1, u64::from(prev.remaining));
            assert_eq!(next.https, https);
            assert!(!prev.https || next.https);
            assert_eq!(next.user_headers(), prev.user_headers() && same_origin);
            assert_eq!(next.body(), prev.body() && preserve);
            assert!(same_origin || (!next.user_headers() && !next.body() && !other_method));
        }
    }

    #[kani::proof]
    fn all_and_only_permitted_steps_succeed() {
        let prev = arbitrary_state();
        let same_origin = kani::any();
        let https = kani::any();
        let other_method: bool = kani::any();
        let drop_body: bool = kani::any();
        let mode = if drop_body {
            Redirect::DropBody
        } else {
            Redirect::PreserveBody
        };
        assert_eq!(prev.exhausted(), prev.remaining == 0);
        let read_only = !other_method && (drop_body || !prev.body());
        let permit = prev.remaining != 0 && (!prev.https || https) && (same_origin || read_only);
        assert_eq!(
            prev.follow(mode, same_origin, https, other_method).is_ok(),
            permit
        );
    }

    #[kani::proof]
    fn budget_matches_mathematical_sum() {
        let used: usize = kani::any();
        let add: usize = kani::any();
        let limit: usize = kani::any();
        // On the verified 64-bit target, this is an exact natural-number sum.
        let sum = used as u128 + add as u128;
        match charge(used, add, limit) {
            Some(n) => {
                assert_eq!(n as u128, sum);
                assert!(n <= limit);
            }
            None => {
                assert!(sum > limit as u128);
            }
        }
    }

    #[kani::proof]
    fn download_prefix_conserves_budget() {
        let before: u64 = kani::any();
        let chunk: usize = kani::any();
        let mut left = before;
        let written = take_prefix(&mut left, chunk);
        assert!(written <= chunk);
        assert_eq!(u128::from(left) + written as u128, u128::from(before));
        assert_eq!(written == chunk, chunk as u128 <= u128::from(before));
        if written < chunk {
            assert_eq!(left, 0);
        }
    }
}
