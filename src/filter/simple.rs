use super::*;

#[derive(Clone)]
pub struct Contain<'a> {
    text: &'a str,
}

impl Contain<'_> {
    pub fn new<'a>(text: &'a str) -> Box<Contain<'a>> {
        Box::new(Contain { text })
    }
}

impl MessageFilter for Contain<'_> {
    fn check_filter(&self, m: &Message) -> bool {
        m.text
            .as_ref()
            .map(|t| t.contains(self.text))
            .unwrap_or(false)
    }
}
