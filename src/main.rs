extern crate comrak;
extern crate git_version;
extern crate htmlescape;
extern crate log;
extern crate orca;
extern crate simple_logger;

use std::collections::VecDeque;
use std::time::Duration;

const VERSION: &str = git_version::git_describe!("--always", "--dirty");

fn comrak_opts() -> comrak::ComrakOptions {
    comrak::ComrakOptions {
        ..comrak::ComrakOptions::default()
    }
}

fn contains_fenced_block<'a>(body: &str) -> bool {
    let arena = comrak::Arena::new();
    let ast = comrak::parse_document(&arena, body, &comrak_opts());
    for node in ast.descendants() {
        let mut n = node.data.borrow_mut();
        match (*n).value {
            comrak::nodes::NodeValue::CodeBlock(ref mut block) => {
                if block.fenced {
                    return true;
                }
            }
            _ => (),
        }
    }
    return false;
}

fn get_pass(p: &str) -> String {
    let res = std::process::Command::new("pass")
        .arg("show")
        .arg(p)
        .output()
        .unwrap();
    if !res.status.success() {
        panic!("pass failed");
    }
    let pass = String::from_utf8(res.stdout)
        .unwrap()
        .lines()
        .next()
        .unwrap()
        .to_owned();
    if pass.is_empty() {
        panic!("pass is empty");
    }
    pass
}

fn strip_type(s: &str) -> &str {
    if s.starts_with("t3_") {
        &s[3..]
    } else {
        s
    }
}

fn find_comment<'a>(
    comments: &'a orca::data::Listing<orca::data::Comment>,
    comment_id: &str,
) -> Option<&'a orca::data::Comment> {
    for c in &comments.children {
        if c.id == comment_id {
            return Some(c);
        }
    }
    for c in &comments.children {
        match find_comment(&c.replies, comment_id) {
            Some(x) => return Some(x),
            None => (),
        }
    }
    None
}

struct MultiSubreddit<'a> {
    app: &'a orca::App,
    names: &'static [&'static str],
    caches: Vec<VecDeque<orca::data::Comment>>,
    last_comment_names: Vec<Option<String>>,
}

impl<'a> MultiSubreddit<'a> {
    fn new(app: &'a orca::App, names: &'static [&'static str]) -> MultiSubreddit<'a> {
        MultiSubreddit {
            app,
            names,
            caches: vec![VecDeque::new(); names.len()],
            last_comment_names: vec![None; names.len()],
        }
    }
    fn refresh(&mut self) {
        let mut fails = 0;
        for (idx, subreddit) in self.names.iter().enumerate() {
            let last = self.last_comment_names[idx].as_ref().map(|s| s.as_str());
            let res: orca::data::Listing<orca::data::Comment> = loop {
                match self.app.get_recent_comments(subreddit, Some(100), last) {
                    Ok(x) => {
                        if fails > 0 {
                            fails = fails - 1;
                        }
                        break x;
                    }
                    Err(e) => {
                        println!(
                            "Error get_recent_comments({:?}, {:?}): {}",
                            subreddit, last, e
                        );
                        use rand::Rng;
                        std::thread::sleep(Duration::from_millis(
                            1000 * fails + rand::thread_rng().gen_range(0, 1000),
                        ));
                        fails = (fails + 1).min(10);
                    }
                }
            };
            if let Some(c) = res.children.front() {
                self.last_comment_names[idx] = Some(c.name.clone());
            }
            // get_recent_comments returns reverse-chronological order, so unreverse it.
            self.caches[idx].extend(res.children.into_iter().rev());
        }
    }
}

impl Iterator for MultiSubreddit<'_> {
    type Item = orca::data::Comment;
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let mut created_utc = None;
            let mut min_idx = None;
            for (idx, cache) in self.caches.iter().enumerate() {
                if let Some(c) = cache.front() {
                    if created_utc.is_none() || c.created_utc < created_utc.unwrap() {
                        min_idx = Some(idx);
                        created_utc = Some(c.created_utc);
                    }
                }
            }
            if let Some(min_idx) = min_idx {
                return self.caches[min_idx].pop_front();
            }
            // We are empty
            self.refresh();
        }
    }
}

fn process_comments(
    username: &str,
    max_age: Duration,
    app: &orca::App,
    comments: &mut MultiSubreddit,
) {
    for comment in comments {
        use std::convert::TryFrom;
        let created = std::time::SystemTime::UNIX_EPOCH
            .checked_add(std::time::Duration::from_secs(
                u64::try_from(comment.created_utc as i64).unwrap(),
            ))
            .unwrap();
        let age = created.elapsed().unwrap();
        println!("https://www.reddit.com{} {:?}", comment.permalink, age);
        // Never reply to ourselves
        if comment.author == username {
            continue;
        }
        // Reddit responses are entity encoded for legacy reasons unless the client passes
        // raw_json=1, which orca doesn't.
        let body = match htmlescape::decode_html(&comment.body) {
            Err(e) => {
                println!("Error: {:?}", e);
                continue;
            }
            Ok(x) => x,
        };
        if !contains_fenced_block(&body) {
            continue;
        }
        // This comment from the comments stream doesn't include replies, so let's load the
        // whole tree.
        let tree = loop {
            match app.get_comment_tree(strip_type(&comment.link_id)) {
                Err(e) => {
                    println!("Error: {:?}", e);
                    continue;
                }
                Ok(x) => break x,
            }
        };
        let tree_comment = find_comment(&tree, &comment.id);
        if let Some(tree_comment) = tree_comment {
            let mut already_replied = false;
            for reply in &tree_comment.replies.children {
                if reply.author == username {
                    already_replied = true;
                    break;
                }
            }
            // Don't reply to the same comment again.
            if already_replied {
                continue;
            }
        } else {
            // Maybe it was deleted?
            println!(
                "Could not find comment {} on link {}",
                comment.id, comment.link_id
            );
            continue;
        }
        if age > max_age {
            continue;
        }
        println!("{}", body);
        let reply = "Your comment uses one or more fenced code blocks (e.g. a block surrounded with ```` ``` ````). These don't render correctly in old reddit even if you authored them in new reddit. Please use code blocks indented with 4 spaces instead. See [my page](https://github.com/singron/old-reddit-fmt-bot/blob/master/about.md) for easy ways to do this and for information and source code for this bot.";
        println!("{}", reply);
        if let Err(e) = app.comment(reply, &comment.name) {
            println!("Error in comment: {}", e);
        }
    }
}

fn main() {
    simple_logger::init_with_level(log::Level::Warn).unwrap();
    let secret = get_pass("Reddit/old-reddit-fmt-bot/secret");
    let id = get_pass("Reddit/old-reddit-fmt-bot/id");
    let password = get_pass("Misc/reddit.com/old-reddit-fmt-bot");
    let mut app = orca::App::new("old fmt experiment", VERSION, "singron").unwrap();
    let username = "old-reddit-fmt-bot";
    app.authorize_script(&id, &secret, username, &password)
        .unwrap();
    // Remove secrets from memory
    drop(secret);
    drop(id);
    drop(password);

    let max_age = std::time::Duration::from_secs(60 * 60 * 24); // 24h
    let mut multi = MultiSubreddit::new(&app, &["programming", "rust", "NixOS"]);
    process_comments(username, max_age, &app, &mut multi);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fenced_block() {
        let tests: &[(bool, &'static str)] = &[
            (true, "```\nhi\n```"),
            (true, "x\n```\nhi\n```"),
            (true, "```rust\nhi\n```"),
            (true, "> ```\n> hi\n> ```"),
            (true, "1.  hi\n    \n    ```\n    hi\n    ```\n"),
            (true, "```\n&\n```"),
            (false, ""),
            (false, "hi\n"),
            (false, "inline `codeblock`\n"),
            (false, "`code`\n"),
            (false, "    hi\n"),
            (false, ">     hi\n"),
        ];
        for (contains, body) in tests {
            assert_eq!(*contains, contains_fenced_block(body));
        }
    }
}
