use jieba_rs::Jieba;
use std::{collections::HashSet, sync::LazyLock};

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

static JIEBA: LazyLock<Jieba> = LazyLock::new(Jieba::new);
static POS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    hashset![
        "v", "l", "n", "nr", "a", "vd", "nz", "PER", "ns", "LOC", "s", "nt", "ORG", "nw", "vn"
    ]
});

pub fn text_cut(text: &str) -> Vec<String> {
    JIEBA
        .tag(text, true)
        .iter()
        .filter(|x| POS.contains(x.tag) && x.word.chars().count() > 1)
        .map(|x| x.word.to_string())
        .collect()
}
