// Horses demo file for builderbot project
// This demonstrates Rust functionality with a horse theme

use std::collections::HashMap;

fn main() {
    println!("Welcome to the Horse Demo!");

    // Demonstrate vector operations with horse breeds
    let breeds = vec!["Arabian", "Thoroughbred", "Quarter Horse", "Mustang", "Clydesdale"];
    println!("Horse breeds count: {}", breeds.len());
    println!("Breeds: {:?}", breeds);

    // Demonstrate string handling
    let stable_name = String::from("Sunset Stables");
    println!("Stable: {}", stable_name);

    // Demonstrate struct usage
    let spirit = Horse {
        name: String::from("Spirit"),
        breed: String::from("Mustang"),
        age: 5,
        color: String::from("Buckskin"),
    };
    println!("Horse: {} - {} {} (age {})", spirit.name, spirit.color, spirit.breed, spirit.age);

    // Demonstrate HashMap with horse care schedule
    let mut feeding_times = HashMap::new();
    feeding_times.insert(String::from("Morning"), String::from("6:00 AM"));
    feeding_times.insert(String::from("Evening"), String::from("6:00 PM"));
    println!("Feeding schedule: {:?}", feeding_times);

    // Demonstrate error handling with horse capacity
    match add_horse_to_stable(15, 20) {
        Ok(new_count) => println!("Successfully added horse. Total horses: {}", new_count),
        Err(e) => println!("Error: {}", e),
    }

    match add_horse_to_stable(20, 20) {
        Ok(new_count) => println!("Successfully added horse. Total horses: {}", new_count),
        Err(e) => println!("Error: {}", e),
    }
}

struct Horse {
    name: String,
    breed: String,
    age: u32,
    color: String,
}

impl Horse {
    fn description(&self) -> String {
        format!("{} is a {} year old {} {}", self.name, self.age, self.color, self.breed)
    }
}

fn add_horse_to_stable(current_count: u32, capacity: u32) -> Result<u32, String> {
    if current_count >= capacity {
        Err(String::from("Stable is at full capacity"))
    } else {
        Ok(current_count + 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_horse_creation() {
        let horse = Horse {
            name: String::from("Thunder"),
            breed: String::from("Arabian"),
            age: 7,
            color: String::from("Black"),
        };
        assert_eq!(horse.name, "Thunder");
        assert_eq!(horse.age, 7);
    }

    #[test]
    fn test_horse_description() {
        let horse = Horse {
            name: String::from("Star"),
            breed: String::from("Thoroughbred"),
            age: 4,
            color: String::from("Bay"),
        };
        assert_eq!(horse.description(), "Star is a 4 year old Bay Thoroughbred");
    }

    #[test]
    fn test_add_horse_success() {
        assert_eq!(add_horse_to_stable(10, 20), Ok(11));
    }

    #[test]
    fn test_add_horse_at_capacity() {
        assert_eq!(add_horse_to_stable(20, 20), Err(String::from("Stable is at full capacity")));
    }

    #[test]
    fn test_add_horse_over_capacity() {
        assert_eq!(add_horse_to_stable(25, 20), Err(String::from("Stable is at full capacity")));
    }
}
