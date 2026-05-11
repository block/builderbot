/// Returns true if the given porcelain status codes (X, Y) represent a
/// merge conflict state per `git status --porcelain` documentation.
pub fn is_conflicted_status(x: char, y: char) -> bool {
    matches!(
        (x, y),
        ('D', 'D') | ('A', 'U') | ('U', 'D') | ('U', 'A') | ('D', 'U') | ('A', 'A') | ('U', 'U')
    )
}
