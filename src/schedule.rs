const RADIX: usize = 32;

pub fn encode(choices: &[(usize, usize)]) -> String {
    let mut schedule = String::new();

    for &(mut choice, mut len) in choices {
        assert!(choice < len);

        while len > 0 {
            schedule.push(char::from_digit((choice % RADIX) as u32, RADIX as u32).unwrap());
            choice /= RADIX;
            len /= RADIX;
        }
    }

    schedule
}

pub struct Decoder {
    schedule: std::vec::IntoIter<char>,
}

impl Decoder {
    pub fn new(schedule: &str) -> Decoder {
        Decoder {
            schedule: schedule.chars().collect::<Vec<_>>().into_iter(),
        }
    }

    pub fn read(&mut self, mut len: usize) -> usize {
        let mut choice = 0;
        let mut offset = 1;

        while len > 0 {
            let digit = self.schedule.next().map_or(0, |ch| {
                ch.to_digit(std::cmp::min(len, RADIX) as u32)
                    .expect("invalid schedule") as usize
            });

            choice += offset * digit;
            offset *= RADIX;
            len /= RADIX;
        }

        choice
    }
}

#[cfg(test)]
mod tests {
    use super::{encode, Decoder};

    fn encode_decode(choices: &[(usize, usize)]) {
        let mut dec = Decoder::new(&encode(choices));

        for &(choice, len) in choices {
            assert_eq!(choice, dec.read(len))
        }
    }

    #[test]
    fn empty() {
        encode_decode(&[])
    }

    #[test]
    fn one() {
        encode_decode(&[(1, 2)])
    }

    #[test]
    fn two() {
        encode_decode(&[(1, 2), (4, 5)]);
    }

    #[test]
    fn long() {
        encode_decode(&[(1, 2), (4, 5), (0, 1), (10, 11), (2, 3), (0, 9)]);
    }

    #[test]
    fn big() {
        encode_decode(&[(12312312, 1231231234)]);
    }
}
