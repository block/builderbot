// Demo file for builderbot project
// This demonstrates enhanced Rust functionality

use std::collections::HashMap;

fn main() {
    println!("Hello from builderbot demo!");

    // Demonstrate basic operations
    let numbers = vec![1, 2, 3, 4, 5];
    let sum: i32 = numbers.iter().sum();
    println!("Sum of {:?} = {}", numbers, sum);

    // Demonstrate string handling
    let message = String::from("This is an enhanced demo");
    println!("{}", message);

    // Demonstrate struct usage
    let person = Person {
        name: String::from("Builder Bot"),
        age: 1,
    };
    println!("Person: {} (age {})", person.name, person.age);

    // Demonstrate HashMap
    let mut scores = HashMap::new();
    scores.insert(String::from("Team A"), 95);
    scores.insert(String::from("Team B"), 88);
    println!("Scores: {:?}", scores);

    // Demonstrate error handling
    match divide(10, 2) {
        Ok(result) => println!("10 / 2 = {}", result),
        Err(e) => println!("Error: {}", e),
    }
}

struct Person {
    name: String,
    age: u32,
}

fn divide(a: i32, b: i32) -> Result<i32, String> {
    if b == 0 {
        Err(String::from("Division by zero"))
    } else {
        Ok(a / b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_math() {
        assert_eq!(2 + 2, 4);
    }

    #[test]
    fn test_divide_success() {
        assert_eq!(divide(10, 2), Ok(5));
    }

    #[test]
    fn test_divide_by_zero() {
        assert_eq!(divide(10, 0), Err(String::from("Division by zero")));
    }

    #[test]
    fn test_person_creation() {
        let person = Person {
            name: String::from("Test"),
            age: 25,
        };
        assert_eq!(person.name, "Test");
        assert_eq!(person.age, 25);
    }
}
