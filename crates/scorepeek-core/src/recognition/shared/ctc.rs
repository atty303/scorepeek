use std::collections::BTreeMap;

struct TrieNode<T> {
    token: u32,
    parent: usize,
    children: BTreeMap<u32, usize>,
    values: Vec<T>,
}

impl<T> Default for TrieNode<T> {
    fn default() -> Self {
        Self {
            token: 0,
            parent: 0,
            children: BTreeMap::new(),
            values: Vec::new(),
        }
    }
}

pub(super) struct CtcSequenceTrie<T> {
    nodes: Vec<TrieNode<T>>,
}

pub(super) struct CtcSequenceScores<'a, T> {
    pub blank_log_probability: f64,
    pub values: Vec<(&'a T, f64)>,
}

impl<T> Default for CtcSequenceTrie<T> {
    fn default() -> Self {
        Self {
            nodes: vec![TrieNode::default()],
        }
    }
}

impl<T> CtcSequenceTrie<T> {
    pub fn insert(&mut self, tokens: &[u32], value: T) -> bool {
        if tokens.is_empty() || tokens.contains(&0) {
            return false;
        }
        let mut node = 0;
        for &token in tokens {
            node = if let Some(child) = self.nodes[node].children.get(&token) {
                *child
            } else {
                let child = self.nodes.len();
                self.nodes.push(TrieNode {
                    token,
                    parent: node,
                    ..TrieNode::default()
                });
                self.nodes[node].children.insert(token, child);
                child
            };
        }
        self.nodes[node].values.push(value);
        true
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.iter().all(|node| node.values.is_empty())
    }

    pub fn score(&self, probabilities: &[f32], classes: usize) -> Option<CtcSequenceScores<'_, T>> {
        if classes == 0
            || probabilities.is_empty()
            || !probabilities.len().is_multiple_of(classes)
            || self
                .nodes
                .iter()
                .skip(1)
                .any(|node| node.token as usize >= classes)
            || probabilities
                .iter()
                .any(|value| !value.is_finite() || *value < 0.0)
        {
            return None;
        }
        let mut blank = vec![f64::NEG_INFINITY; self.nodes.len()];
        let mut nonblank = vec![f64::NEG_INFINITY; self.nodes.len()];
        blank[0] = 0.0;
        for row in probabilities.chunks_exact(classes) {
            let mut next_blank = vec![f64::NEG_INFINITY; self.nodes.len()];
            let mut next_nonblank = vec![f64::NEG_INFINITY; self.nodes.len()];
            let blank_probability = log_probability(row[0]);
            next_blank[0] = blank[0] + blank_probability;
            for index in 1..self.nodes.len() {
                next_blank[index] = logsumexp([blank[index], nonblank[index]]) + blank_probability;
                let node = &self.nodes[index];
                let parent = &self.nodes[node.parent];
                let mut sources = [f64::NEG_INFINITY; 3];
                sources[0] = nonblank[index];
                sources[1] = blank[node.parent];
                if node.token != parent.token || node.parent == 0 {
                    sources[2] = nonblank[node.parent];
                }
                next_nonblank[index] =
                    logsumexp(sources) + log_probability(row[node.token as usize]);
            }
            blank = next_blank;
            nonblank = next_nonblank;
        }
        let scores: Vec<_> = blank
            .into_iter()
            .zip(nonblank)
            .map(|(blank, nonblank)| logsumexp([blank, nonblank]))
            .collect();
        let values = self
            .nodes
            .iter()
            .zip(&scores)
            .flat_map(|(node, score)| node.values.iter().map(move |value| (value, *score)))
            .collect();
        Some(CtcSequenceScores {
            blank_log_probability: scores[0],
            values,
        })
    }
}

fn log_probability(value: f32) -> f64 {
    if value == 0.0 {
        f64::NEG_INFINITY
    } else {
        f64::from(value).ln()
    }
}

fn logsumexp<const N: usize>(values: [f64; N]) -> f64 {
    let maximum = values.into_iter().fold(f64::NEG_INFINITY, f64::max);
    if maximum == f64::NEG_INFINITY {
        maximum
    } else {
        maximum
            + values
                .into_iter()
                .map(|value| (value - maximum).exp())
                .sum::<f64>()
                .ln()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scores_blank_repeats_and_shared_prefixes_exactly() {
        let mut trie = CtcSequenceTrie::default();
        assert!(trie.insert(&[1], "1"));
        assert!(trie.insert(&[1, 1], "11"));
        let probabilities = [
            0.6_f32, 0.4, // blank or 1
            0.2, 0.8, // 1
            0.7, 0.3, // blank separator
            0.2, 0.8, // 1 again
        ];
        let scores = trie.score(&probabilities, 2).unwrap();
        assert_eq!(scores.values.len(), 2);
        assert!(scores.values[1].1 > scores.values[0].1);
        assert!(scores.values[0].1 > scores.blank_log_probability);
    }
}
