//! The period hint: whether an observation line names a time. NLI reads two
//! sentences as describing the same moment, so two true lines from different
//! periods read as a contradiction; a line that names a period makes the
//! finding point at the validity window. A hint only: it never filters.
//!
//! A hand scanner, because the index crate carries no regex dependency. It
//! misfires on the month words that are also ordinary words ("may", "march",
//! "mai") and on a decimal before a full stop ("1.5."); the measurement
//! reports its hit rate.

/// Month names, English and German, lowercased.
const MONTHS: &[&str] = &[
    "january",
    "february",
    "march",
    "april",
    "may",
    "june",
    "july",
    "august",
    "september",
    "october",
    "november",
    "december",
    "januar",
    "jänner",
    "februar",
    "märz",
    "mai",
    "juni",
    "juli",
    "oktober",
    "dezember",
];

/// Single words that bound a period, English and German.
const PERIOD_WORDS: &[&str] = &["since", "until", "till", "seit", "bis", "ab"];

/// Two-word phrases that bound a period.
const PERIOD_PHRASES: &[[&str; 2]] = &[["as", "of"], ["no", "longer"], ["nicht", "mehr"]];

/// Whether `text` contains a year from 1900 to 2099, an ISO date, a German
/// day and month (`27.09.2026`, `27.9.`), a month name or one of the period
/// words and phrases.
pub fn names_period(text: &str) -> bool {
    let runs = digit_runs(text);
    if runs
        .iter()
        .any(|&(s, e)| e - s == 4 && matches!(text[s..e].parse::<u32>(), Ok(1900..=2099)))
    {
        return true;
    }
    let bytes = text.as_bytes();
    // A day 1 to 31 and a month 1 to 12, each followed by a full stop. A
    // run that goes on after the second stop ("18.19.0", "1.2.3") or that
    // follows a digit and a stop ("10.0.0.1") is a version or an address.
    let day_month = runs.windows(2).any(|w| {
        let ((s1, e1), (s2, e2)) = (w[0], w[1]);
        (1..=2).contains(&(e1 - s1))
            && (1..=2).contains(&(e2 - s2))
            && bytes.get(e1) == Some(&b'.')
            && s2 == e1 + 1
            && bytes.get(e2) == Some(&b'.')
            && matches!(text[s1..e1].parse::<u32>(), Ok(1..=31))
            && matches!(text[s2..e2].parse::<u32>(), Ok(1..=12))
            && !bytes.get(e2 + 1).is_some_and(u8::is_ascii_digit)
            && !(s1 >= 2 && bytes[s1 - 1] == b'.' && bytes[s1 - 2].is_ascii_digit())
    });
    if day_month {
        return true;
    }
    let words: Vec<String> = text
        .split(|c: char| !c.is_alphabetic())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect();
    words
        .iter()
        .any(|w| MONTHS.contains(&w.as_str()) || PERIOD_WORDS.contains(&w.as_str()))
        || words.windows(2).any(|p| {
            PERIOD_PHRASES
                .iter()
                .any(|ph| p[0] == ph[0] && p[1] == ph[1])
        })
}

/// The byte ranges of every maximal run of ASCII digits.
fn digit_runs(text: &str) -> Vec<(usize, usize)> {
    let mut runs = Vec::new();
    let mut start: Option<usize> = None;
    for (i, b) in text.bytes().enumerate() {
        match (b.is_ascii_digit(), start) {
            (true, None) => start = Some(i),
            (false, Some(s)) => {
                runs.push((s, i));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        runs.push((s, text.len()));
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::names_period;

    #[test]
    fn dated_and_bounded_lines_name_a_period() {
        for line in [
            "Since 2024 the build uses Node 20",
            "The mirror moved on 2026-09-27",
            "Seit dem 27.09.2026 läuft der Build nachts",
            "Gilt bis 27.9.",
            "Ab März nutzt das Lager den neuen Scanner",
            "As of June the queue drains hourly",
            "The build no longer uses Node 18",
            "Der Server braucht nicht mehr neu zu starten",
            "until the migration lands",
        ] {
            assert!(names_period(line), "{line}");
        }
    }

    #[test]
    fn ordinary_lines_and_german_numbers_do_not() {
        for line in [
            "The build uses Node 18",
            "Der Puffer fasst 1.000 Einträge",
            "Die Toleranz liegt bei 1,5 mm",
            "The mount swings thirty degrees",
            "The key is 12345 long",
        ] {
            assert!(!names_period(line), "{line}");
        }
    }

    /// Version strings and addresses are two short digit runs with stops,
    /// like a German day and month, but never a period.
    #[test]
    fn versions_and_addresses_do_not_name_a_period() {
        for line in [
            "The build uses Node 18.19.0",
            "The gateway answers on 10.0.0.1",
            "The client pins v1.2.3",
            "The mirror serves 192.168.1.20",
            "Die Vorlage liegt unter 40.13.",
        ] {
            assert!(!names_period(line), "{line}");
        }
        for line in [
            "Gilt bis 27.9.",
            "Seit dem 27.09.2026",
            "Am 1.12. zieht das Lager um",
        ] {
            assert!(names_period(line), "{line}");
        }
    }

    /// The known misfires, pinned so the behaviour is explicit: a month word
    /// used as an ordinary word, and a decimal before a full stop, both read
    /// as a period. The hint only adds a sentence to a finding, so a false
    /// positive costs little; the measurement reports the hit rate.
    #[test]
    fn known_misfires_read_as_a_period() {
        for line in [
            "You may restart the server at any time",
            "The crews march to the depot",
            "Das Mai-Paket liegt im Lager",
            "The ratio is 1.5.",
        ] {
            assert!(names_period(line), "{line}");
        }
        assert!(
            !names_period("The ratio is 1.5"),
            "without the final stop it is a number"
        );
        assert!(
            !names_period("The ratio is 1.5 today."),
            "a decimal mid-line is a number"
        );
    }
}
