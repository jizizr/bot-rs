use super::*;
use crate::getor;
trait MessageFilter {
    fn check_filter(&self, m: &Message) -> bool;
}

#[derive(Clone)]
pub struct Contain<'a>(&'a str);

impl Contain<'_> {
    #[allow(dead_code)]
    pub fn new(text: &str) -> Box<Contain<'_>> {
        Box::new(Contain(text))
    }
}

impl MessageFilter for Contain<'_> {
    fn check_filter(&self, m: &Message) -> bool {
        getor(m).map(|t| t.contains(self.0)).unwrap_or(false)
    }
}

#[derive(Clone)]
pub struct Equal(String);

impl Equal {
    #[allow(dead_code)]
    pub fn new(text: &str) -> Box<Equal> {
        Box::new(Equal(text.to_string()))
    }
}

impl MessageFilter for Equal {
    fn check_filter(&self, m: &Message) -> bool {
        getor(m).map(|t| *t == self.0).unwrap_or(false)
    }
}
