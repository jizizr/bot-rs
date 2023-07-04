use super::*;

#[derive(Clone)]
pub struct Contain<'a>(&'a str);

impl Contain<'_> {
    pub fn new<'a>(text: &'a str) -> Box<Contain<'a>> {
        Box::new(Contain(text))
    }
}

impl MessageFilter for Contain<'_> {
    fn check_filter(&self, m: &Message) -> bool {
        m.text.as_ref().map(|t| t.contains(self.0)).unwrap_or(false)
    }
}

#[derive(Clone)]
pub struct Equal(String);

impl Equal {
    pub fn new<'a>(text: &str) -> Box<Equal> {
        Box::new(Equal(text.to_string()))
    }
}

impl MessageFilter for Equal {
    fn check_filter(&self, m: &Message) -> bool {
        m.text.as_ref().map(|t| *t == self.0).unwrap_or(false)
    }
}
