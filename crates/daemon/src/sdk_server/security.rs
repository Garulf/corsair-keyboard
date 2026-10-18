pub fn pipe_sddl(user_sid: &str) -> String {
    format!("D:P(A;;GA;;;{user_sid})(A;;GA;;;SY)")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pipe_dacl_allows_only_the_user_and_system() {
        assert_eq!(pipe_sddl("S-1-5-21-1-2-3-1001"), "D:P(A;;GA;;;S-1-5-21-1-2-3-1001)(A;;GA;;;SY)");
    }
}
