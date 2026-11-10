//! The `=` calculator: a small recursive-descent evaluator.
//!
//! Supports `+ - * / ^` (also `×`, `÷`, `**`), `a % b` for b percent of a
//! (`100%10` = 10), `a mod b` for the remainder, parentheses, unary minus,
//! implicit multiplication after a number (`2pi`, `3(4+1)`), the constants
//! `pi`, `tau` and `e`, and the functions `sqrt cbrt abs exp ln log log2 sin
//! cos tan asin acos atan floor ceil round`.

pub fn eval(input: &str) -> Option<f64> {
    let mut parser = Parser {
        chars: input.chars().collect(),
        pos: 0,
    };
    let value = parser.expr()?;
    (parser.peek().is_none() && value.is_finite()).then_some(value)
}

/// Formats a result for display and copying: integers without a decimal
/// point, up to 10 decimal places otherwise, scientific for extremes.
pub fn format(value: f64) -> String {
    let value = if value == 0.0 { 0.0 } else { value }; // no "-0"
    let magnitude = value.abs();
    if magnitude != 0.0 && !(1e-6..1e15).contains(&magnitude) {
        return format!("{value:e}");
    }
    let text = format!("{value:.10}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    text.to_string()
}

struct Parser {
    chars: Vec<char>,
    pos: usize,
}

impl Parser {
    /// The next character after any whitespace.
    fn peek(&mut self) -> Option<char> {
        while self.chars.get(self.pos).is_some_and(|c| c.is_whitespace()) {
            self.pos += 1;
        }
        self.chars.get(self.pos).copied()
    }

    /// Whether the next token is exactly `word` (not the start of a longer name).
    fn at_word(&self, word: &str) -> bool {
        let len = word.len();
        let next: String = self.chars.iter().skip(self.pos).take(len).collect();
        next.eq_ignore_ascii_case(word) && !self.raw(len).is_some_and(|c| c.is_alphabetic())
    }

    /// The character `offset` places ahead, without skipping whitespace.
    fn raw(&self, offset: usize) -> Option<char> {
        self.chars.get(self.pos + offset).copied()
    }

    fn eat(&mut self, c: char) -> bool {
        let found = self.peek() == Some(c);
        self.pos += found as usize;
        found
    }

    /// expr = term (('+' | '-') term)*
    fn expr(&mut self) -> Option<f64> {
        let mut value = self.term()?;
        loop {
            if self.eat('+') {
                value += self.term()?;
            } else if self.eat('-') || self.eat('−') {
                value -= self.term()?;
            } else {
                return Some(value);
            }
        }
    }

    /// term = unary (('*' | '/' | '%' | "mod") unary | implicit factor)*
    fn term(&mut self) -> Option<f64> {
        let mut value = self.unary()?;
        loop {
            match self.peek() {
                Some('*') if self.raw(1) == Some('*') => return Some(value),
                Some('*' | '×' | 'x') => {
                    self.pos += 1;
                    value *= self.unary()?;
                }
                Some('/' | '÷') => {
                    self.pos += 1;
                    value /= self.unary()?;
                }
                // a % b: b percent of a.
                Some('%') => {
                    self.pos += 1;
                    value = value * self.unary()? / 100.0;
                }
                Some('m' | 'M') if self.at_word("mod") => {
                    self.pos += 3;
                    value %= self.unary()?;
                }
                // 2pi, 3(4+1), (1+2)(3+4)
                Some(c) if c == '(' || c.is_alphabetic() => value *= self.power()?,
                _ => return Some(value),
            }
        }
    }

    /// unary = '-' unary | '+' unary | power. Binds looser than '^', so -2^2 = -4.
    fn unary(&mut self) -> Option<f64> {
        if self.eat('-') || self.eat('−') {
            Some(-self.unary()?)
        } else if self.eat('+') {
            self.unary()
        } else {
            self.power()
        }
    }

    /// power = primary (('^' | '**') unary)?, right-associative.
    fn power(&mut self) -> Option<f64> {
        let base = self.primary()?;
        let starred = self.peek() == Some('*') && self.raw(1) == Some('*');
        if starred || self.eat('^') {
            self.pos += 2 * starred as usize;
            return Some(base.powf(self.unary()?));
        }
        Some(base)
    }

    fn primary(&mut self) -> Option<f64> {
        let c = self.peek()?;
        if self.eat('(') {
            let value = self.expr()?;
            self.eat(')'); // tolerate a missing closing parenthesis while typing
            return Some(value);
        }
        if c.is_ascii_digit() || c == '.' {
            return self.number();
        }
        if c == 'π' {
            self.pos += 1;
            return Some(std::f64::consts::PI);
        }
        // 'x' is multiplication, so a name never starts with it.
        if c == 'x' {
            return None;
        }
        let start = self.pos;
        while self.raw(0).is_some_and(|c| c.is_ascii_alphabetic()) {
            self.pos += 1;
        }
        if self.chars[start..self.pos]
            .iter()
            .collect::<String>()
            .eq_ignore_ascii_case("log")
            && self.raw(0) == Some('2')
        {
            self.pos += 1;
        }
        let name: String = self.chars[start..self.pos]
            .iter()
            .collect::<String>()
            .to_lowercase();
        let constant = match name.as_str() {
            "pi" => Some(std::f64::consts::PI),
            "tau" => Some(std::f64::consts::TAU),
            "e" => Some(std::f64::consts::E),
            _ => None,
        };
        if constant.is_some() {
            return constant;
        }
        let f: fn(f64) -> f64 = match name.as_str() {
            "sqrt" => f64::sqrt,
            "cbrt" => f64::cbrt,
            "abs" => f64::abs,
            "exp" => f64::exp,
            "ln" => f64::ln,
            "log" => f64::log10,
            "log2" => f64::log2,
            "sin" => f64::sin,
            "cos" => f64::cos,
            "tan" => f64::tan,
            "asin" => f64::asin,
            "acos" => f64::acos,
            "atan" => f64::atan,
            "floor" => f64::floor,
            "ceil" => f64::ceil,
            "round" => f64::round,
            _ => return None,
        };
        Some(f(self.power()?))
    }

    fn number(&mut self) -> Option<f64> {
        let start = self.pos;
        while self
            .raw(0)
            .is_some_and(|c| c.is_ascii_digit() || c == '.' || c == '_')
        {
            self.pos += 1;
        }
        // Exponent: 1e3, 2.5e-4. A bare 'e' after a number is Euler's number (2e = 2·e).
        if self.raw(0) == Some('e') {
            let mut ahead = self.pos + 1;
            if matches!(self.chars.get(ahead), Some('+' | '-')) {
                ahead += 1;
            }
            if self.chars.get(ahead).is_some_and(|c| c.is_ascii_digit()) {
                self.pos = ahead;
                while self.raw(0).is_some_and(|c| c.is_ascii_digit()) {
                    self.pos += 1;
                }
            }
        }
        let text: String = self.chars[start..self.pos]
            .iter()
            .filter(|&&c| c != '_')
            .collect();
        text.parse().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn calc(s: &str) -> Option<String> {
        eval(s).map(format)
    }

    #[test]
    fn arithmetic() {
        assert_eq!(calc("12*4.5").as_deref(), Some("54"));
        assert_eq!(calc("1 + 2 * 3").as_deref(), Some("7"));
        assert_eq!(calc("(1+2)*3").as_deref(), Some("9"));
        assert_eq!(calc("10/4").as_deref(), Some("2.5"));
        assert_eq!(calc("100%10").as_deref(), Some("10"));
        assert_eq!(calc("2%10").as_deref(), Some("0.2"));
        assert_eq!(calc("200 % 15 + 1").as_deref(), Some("31"));
        assert_eq!(calc("17 mod 5").as_deref(), Some("2"));
        assert_eq!(calc("17MOD5").as_deref(), Some("2"));
        assert_eq!(calc("10 % 4").as_deref(), Some("0.4"));
        assert_eq!(calc("2^10").as_deref(), Some("1024"));
        assert_eq!(calc("2**3**2").as_deref(), Some("512"));
        assert_eq!(calc("-2^2").as_deref(), Some("-4"));
        assert_eq!(calc("2^-1").as_deref(), Some("0.5"));
        assert_eq!(calc("3x4").as_deref(), Some("12"));
        assert_eq!(calc("6 ÷ 4 × 2").as_deref(), Some("3"));
        assert_eq!(calc("1_000 * 3").as_deref(), Some("3000"));
        assert_eq!(calc("0.1+0.2").as_deref(), Some("0.3"));
        assert_eq!(calc("-0").as_deref(), Some("0"));
    }

    #[test]
    fn functions_and_constants() {
        assert_eq!(calc("sqrt(16)").as_deref(), Some("4"));
        assert_eq!(calc("sqrt 2").as_deref(), Some("1.4142135624"));
        assert_eq!(calc("log2(8) + log2 8").as_deref(), Some("6"));
        assert_eq!(calc(" 2 ^ 3 ").as_deref(), Some("8"));
        assert_eq!(calc("2pi").as_deref(), Some("6.2831853072"));
        assert_eq!(calc("3(4+1)").as_deref(), Some("15"));
        assert_eq!(calc("log(1000)").as_deref(), Some("3"));
        assert_eq!(calc("ln(e)").as_deref(), Some("1"));
        assert_eq!(calc("1e3").as_deref(), Some("1000"));
        assert_eq!(calc("2.5e-4*4").as_deref(), Some("0.001"));
        assert_eq!(calc("2e").as_deref(), Some("5.4365636569"));
    }

    #[test]
    fn partial_and_invalid() {
        assert_eq!(calc("(1+2").as_deref(), Some("3")); // still typing
        assert_eq!(calc(""), None);
        assert_eq!(calc("1+"), None);
        assert_eq!(calc("foo(2)"), None);
        assert_eq!(calc("1/0"), None);
        assert_eq!(calc("2 3"), None);
        assert_eq!(calc("x2"), None);
    }

    #[test]
    fn extremes() {
        assert_eq!(calc("10^20").as_deref(), Some("1e20"));
        assert_eq!(calc("1/10^8").as_deref(), Some("1e-8"));
    }
}
