// Demo file for builderbot project
// This demonstrates basic Rust functionality

fn main() {
    println!("Hello from builderbot demo!");

    // Demonstrate basic operations
    let numbers = vec![1, 2, 3, 4, 5];
    let sum: i32 = numbers.iter().sum();

    println!("Sum of {:?} = {}", numbers, sum);

    // Demonstrate string handling
    let message = String::from("This is a demo");
    println!("{}", message);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_demo() {
        // Basic test to ensure demo compiles
        assert_eq!(2 + 2, 4);
    }
}
