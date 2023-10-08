use super::*;
use jieba_rs::Jieba;
use std::collections::HashSet;

macro_rules! hashset {
    ($($x:expr),*) => {
        {
            let mut temp_set = HashSet::new();
            $(
                temp_set.insert($x);
            )*
            temp_set
        }
    };
}

lazy_static! {
    static ref JIEBA: Jieba = Jieba::new();
    static ref POS: HashSet<&'static str> = hashset![
        "v", "l", "n", "nr", "a", "vd", "nz", "PER", "ns", "LOC", "s", "nt", "ORG", "nw", "vn"
    ];
}

pub fn text_cut(text: &str) -> Vec<String> {
    JIEBA
        .tag(text, true)
        .iter()
        .filter(|x| POS.contains(x.tag) && x.word.chars().count() > 1)
        .map(|x| x.word.to_string())
        .collect()
}
