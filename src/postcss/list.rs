//! `postcss/lib/list.js`

/// `list.comma(string)`
pub fn comma(s: &str) -> Vec<String> {
    split(s, &[','], true)
}

/// `list.space(string)`
pub fn space(s: &str) -> Vec<String> {
    split(s, &[' ', '\n', '\t'], false)
}

/// `list.split(string, separators, last)`
pub fn split(s: &str, separators: &[char], last: bool) -> Vec<String> {
    let mut array = Vec::new();
    let mut current = String::new();
    let mut split = false;
    let mut func = 0;
    let mut in_quote = false;
    let mut prev_quote = '\0';
    let mut escape = false;

    for letter in s.chars() {
        if escape {
            escape = false;
        } else if letter == '\\' {
            escape = true;
        } else if in_quote {
            if letter == prev_quote {
                in_quote = false;
            }
        } else if letter == '"' || letter == '\'' {
            in_quote = true;
            prev_quote = letter;
        } else if letter == '(' {
            func += 1;
        } else if letter == ')' {
            if func > 0 {
                func -= 1;
            }
        } else if func == 0 && separators.contains(&letter) {
            split = true;
        }

        if split {
            if !current.is_empty() {
                array.push(crate::js::trim(&current).to_string());
            }
            current.clear();
            split = false;
        } else {
            current.push(letter);
        }
    }

    if last || !current.is_empty() {
        array.push(crate::js::trim(&current).to_string());
    }
    array
}
