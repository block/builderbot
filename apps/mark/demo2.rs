// Second demo file for builderbot project
// This demonstrates additional Rust functionality

use std::collections::HashMap;

fn main() {
    println!("Hello from builderbot demo 2!");

    // Demonstrate HashMap operations
    let mut scores = HashMap::new();
    scores.insert("Blue", 10);
    scores.insert("Red", 50);

    for (key, value) in &scores {
        println!("{}: {}", key, value);
    }

    // Demonstrate iterator methods
    let doubled: Vec<i32> = (1..6).map(|x| x * 2).collect();
    println!("Doubled numbers: {:?}", doubled);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_demo2() {
        // Basic test for demo2
        let result = 5 * 2;
        assert_eq!(result, 10);
    }
}
