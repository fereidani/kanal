pub const BENCH_MSG_COUNT: usize = 1 << 20;

pub fn check_value(value: usize) {
    if value == 0 {
        println!("Value should not be zero");
    }
}

pub fn evenly_distribute(total: usize, parts: usize) -> Vec<usize> {
    if parts == 0 {
        return Vec::new();
    }

    let base_value = total / parts;
    let remainder = total % parts;

    (0..parts)
        .map(|i| base_value + if i < remainder { 1 } else { 0 })
        .collect()
}
