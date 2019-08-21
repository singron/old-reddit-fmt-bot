extern crate comrak;
extern crate failure;
extern crate git_version;
extern crate htmlescape;
extern crate log;
extern crate orca;
extern crate simple_logger;

use std::collections::VecDeque;
use std::time::{Duration, Instant};

const DRY_RUN: bool = false;

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
    let b = s.as_bytes();
    if let (Some(b't'), Some(d), Some(b'_')) = (b.get(0), b.get(1), b.get(2)) {
        if d.is_ascii_digit() {
            return &s[3..];
        }
    }
    s
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

struct Backoff {
    fails: u64,
}

impl Backoff {
    fn ok(&mut self) {
        if self.fails > 0 {
            self.fails -= 1;
        }
    }

    fn fail_wait(&mut self) {
        use rand::Rng;
        std::thread::sleep(Duration::from_millis(
            1000 * self.fails + rand::thread_rng().gen_range(0, 1000),
        ));
        self.fails = (self.fails + 1).min(10);
    }

    fn loop_wait<T, E, C: Fn() -> Result<T, E>, H: Fn(E)>(&mut self, call: C, on_err: H) -> T {
        loop {
            match call() {
                Ok(x) => {
                    self.ok();
                    return x;
                }
                Err(e) => on_err(e),
            }
        }
    }
}

struct MadeComment {
    parent_name: String,
    name: String,
    link_id: String,
    edited: bool,
}

fn write_reply(out: &mut String, comment: &orca::data::Comment) {
    use std::fmt::Write;
    write!(
        out,
        "Your comment uses fenced code blocks (e.g. blocks surrounded \n\
         with ```` ``` ````). These don't render correctly in old \n\
         reddit even if you authored them in new reddit. Please use \n\
         code blocks indented with 4 spaces instead. See what the \n\
         comment looks like in \n\
         [new](https://new.reddit.com{permalink}) \n\
         and \n\
         [old](https://old.reddit.com{permalink}) \n\
         reddit. \n\
         [My page](https://github.com/singron/old-reddit-fmt-bot/blob/master/about.md) \n\
         has easy ways to indent code as well as information and source code for this bot.",
        permalink = EscapeMarkdownLink(&comment.permalink),
    )
    .unwrap();
}

struct MultiSubreddit<'a> {
    app: &'a orca::App,
    username: &'a str,
    names: &'static [&'static str],
    caches: Vec<VecDeque<orca::data::Comment>>,
    recent_comment_names: Vec<VecDeque<String>>,
    comments_made: Vec<MadeComment>,
    comments_made_dirty: bool,
    last_comments_made_check: Option<Instant>,
    backoff: Backoff,
    last_refresh: Option<Instant>,
    last_new_comment: Option<Instant>,
}

impl<'a> MultiSubreddit<'a> {
    fn new(
        app: &'a orca::App,
        username: &'a str,
        names: &'static [&'static str],
    ) -> MultiSubreddit<'a> {
        MultiSubreddit {
            app,
            username,
            names,
            caches: vec![VecDeque::new(); names.len()],
            recent_comment_names: vec![VecDeque::new(); names.len()],
            comments_made: Vec::new(),
            comments_made_dirty: true,
            last_comments_made_check: None,
            last_refresh: None,
            last_new_comment: None,
            backoff: Backoff { fails: 0 },
        }
    }

    fn load_comments_made(&mut self) -> Result<(), failure::Error> {
        let mut opts = orca::app::UserListingOpts::default();
        opts.limit(100);
        let comments: orca::data::Listing<orca::data::Comment> =
            self.app.get_user_comments(self.username, &opts)?;
        let mut comments_made = Vec::with_capacity(comments.children.len());
        for comment in comments.children {
            let comment: orca::data::Comment = comment;
            comments_made.push(MadeComment {
                parent_name: comment.parent_id,
                name: comment.name,
                edited: comment.body.contains("EDIT:"),
                link_id: comment.link_id,
            });
        }
        self.comments_made = comments_made;
        self.comments_made_dirty = false;
        Ok(())
    }

    fn refresh(&mut self) {
        if let Some(last_refresh) = self.last_refresh {
            let min_refresh = Duration::from_secs(5);
            let e = last_refresh.elapsed();
            if e < min_refresh {
                std::thread::sleep(min_refresh - e);
            }
        }
        for (idx, subreddit) in self.names.iter().enumerate() {
            if !self.caches[idx].is_empty() {
                continue;
            }
            let res: orca::data::Listing<orca::data::Comment> = loop {
                let recent_comment = self.recent_comment_names[idx].front().map(|s| s.as_str());
                match self
                    .app
                    .get_recent_comments(subreddit, Some(100), recent_comment)
                {
                    Ok(res) => {
                        self.backoff.ok();
                        if res.children.is_empty() && recent_comment.is_some() {
                            // If we try to use a deleted comment as the `before` parameter when
                            // getting recent comments, we will get empty results forever.
                            let name = recent_comment.unwrap();
                            let backoff = &mut self.backoff;
                            let app = &mut self.app;
                            let comment = backoff.loop_wait(
                                || app.get_comment(name),
                                |e| println!("Error in get_comment({:?}): {}", name, e),
                            );
                            if comment.is_none() || comment.unwrap().author == "[deleted]" {
                                // We will use the next most recent comment, or eventually get
                                // another listing from scratch.
                                self.recent_comment_names[idx].pop_front();
                                continue;
                            }
                        }
                        break res;
                    }
                    Err(e) => {
                        println!(
                            "Error get_recent_comments({:?}, {:?}): {}",
                            subreddit, recent_comment, e
                        );
                        self.backoff.fail_wait();
                    }
                }
            };
            let skip = if res.children.len() > 10 {
                res.children.len() - 10
            } else {
                0
            };
            for c in res.children.iter().rev().skip(skip) {
                self.recent_comment_names[idx].push_front(c.name.clone());
            }
            if self.recent_comment_names[idx].len() > 10 {
                self.recent_comment_names[idx].truncate(10);
            }
            // get_recent_comments returns reverse-chronological order, so unreverse it.
            self.caches[idx].extend(res.children.into_iter().rev());
        }
        if self.comments_made_dirty {
            loop {
                match self.load_comments_made() {
                    Ok(x) => {
                        self.backoff.ok();
                        break x;
                    }
                    Err(e) => {
                        println!("Error load_comments_made: {}", e);
                        self.backoff.fail_wait();
                    }
                }
            }
        }
        self.last_refresh = Some(Instant::now());
    }

    fn on_new_comment(&mut self, comment: orca::data::Comment) {
        use std::convert::TryFrom;
        let created = std::time::SystemTime::UNIX_EPOCH
            .checked_add(std::time::Duration::from_secs(
                u64::try_from(comment.created_utc as i64).unwrap(),
            ))
            .unwrap();
        let age = created.elapsed().unwrap();
        println!("https://www.reddit.com{} {:?}", comment.permalink, age);
        // Never reply to ourselves
        if comment.author == self.username {
            return;
        }
        // Reddit responses are entity encoded for legacy reasons unless the client passes
        // raw_json=1, which orca doesn't.
        let body = match htmlescape::decode_html(&comment.body) {
            Err(e) => {
                println!("Error: {:?}", e);
                return;
            }
            Ok(x) => x,
        };
        if !contains_fenced_block(&body) {
            return;
        }
        // This comment from the comments stream doesn't include replies, so let's load the
        // whole tree.
        let tree = loop {
            match self.app.get_comment_tree(strip_type(&comment.link_id)) {
                Err(e) => {
                    println!("Error: {:?}", e);
                    return;
                }
                Ok(x) => break x,
            }
        };
        let tree_comment = find_comment(&tree, &comment.id);
        if let Some(tree_comment) = tree_comment {
            let mut already_replied = false;
            for reply in &tree_comment.replies.children {
                if reply.author == self.username {
                    already_replied = true;
                    break;
                }
            }
            // Don't reply to the same comment again.
            if already_replied {
                return;
            }
        } else {
            // Maybe it was deleted?
            println!(
                "Could not find comment {} on link {}",
                comment.id, comment.link_id
            );
            return;
        }
        let max_age = std::time::Duration::from_secs(60 * 60 * 24); // 24h
        if age > max_age {
            return;
        }
        println!("{}", body);
        let mut reply = String::new();
        write_reply(&mut reply, &comment);
        println!("{}", &reply);
        if DRY_RUN {
            println!("DRY_RUN: not commenting");
            return;
        }
        self.comments_made_dirty = true;
        if let Err(e) = self.app.comment(&reply, &comment.name) {
            println!("Error in comment: {}", e);
        }
    }

    fn check_comments_made(&mut self) {
        for comment_made in &mut self.comments_made {
            if comment_made.edited {
                continue;
            }
            let tree = match self.app.get_comment_tree(strip_type(&comment_made.link_id)) {
                Err(e) => {
                    println!(
                        "Error in get_comment_tree({:?}): {}",
                        comment_made.link_id, e
                    );
                    self.backoff.fail_wait();
                    continue;
                }
                Ok(x) => {
                    self.backoff.ok();
                    x
                }
            };
            let parent_comment = match find_comment(&tree, strip_type(&comment_made.parent_name)) {
                Some(x) => x,
                None => {
                    println!(
                        "Could not find comment {} in {}",
                        comment_made.parent_name, comment_made.link_id
                    );
                    continue;
                }
            };
            if contains_fenced_block(&parent_comment.body) {
                continue;
            }
            // They fixed their comment
            println!(
                "Should edit reply to https://www.reddit.com{}",
                parent_comment.permalink
            );
            let mut new_reply = "EDIT: Thanks for editing your comment!\n\n".to_string();
            write_reply(&mut new_reply, &parent_comment);
            println!("{}", new_reply);
            if DRY_RUN {
                println!("DRY_RUN: not editing")
            } else {
                if let Err(e) = self.app.edit(&new_reply, &comment_made.name) {
                    println!("Error in edit({:?}): {}", &comment_made.name, e);
                }
            }
            comment_made.edited = true;
        }
        self.last_comments_made_check = Some(Instant::now());
    }

    fn process(&mut self) {
        let mut error_mode = 0;
        loop {
            if let Some(last_new_comment) = self.last_new_comment {
                let minutes = last_new_comment.elapsed().as_secs() as f64 / 60.0;
                if minutes > 90.0 {
                    if error_mode != 3 {
                        error_mode = 3;
                        log::error!("Set error mode {}", error_mode);
                        log::set_max_level(log::LevelFilter::Trace);
                    }
                } else if minutes > 60.0 {
                    if error_mode != 2 {
                        error_mode = 2;
                        log::error!("Set error mode {}", error_mode);
                        log::set_max_level(log::LevelFilter::Debug);
                    }
                } else if minutes > 30.0 {
                    if error_mode != 1 {
                        error_mode = 1;
                        log::error!("Set error mode {}", error_mode);
                        log::set_max_level(log::LevelFilter::Info);
                    }
                }
            }
            self.refresh();
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
                    let comment = self.caches[min_idx].pop_front().unwrap();
                    self.last_new_comment = Some(Instant::now());
                    if error_mode != 0 {
                        log::error!("Resetting error mode");
                        log::set_max_level(log::LevelFilter::Warn);
                        error_mode = 0;
                    }
                    self.on_new_comment(comment);
                } else {
                    break;
                }
            }
            if self
                .last_comments_made_check
                .map(|i| i < Instant::now() - Duration::from_secs(5 * 60))
                .unwrap_or(true)
            {
                self.check_comments_made();
            }
        }
    }
}

struct EscapeMarkdownLink<'a>(&'a str);

impl std::fmt::Display for EscapeMarkdownLink<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Based on
        // * https://spec.commonmark.org/0.29/#link-destination
        // * https://www.reddit.com/wiki/markdown#wiki_tips_for_robots
        let mut s = &self.0[..];
        while !s.is_empty() {
            let offset = s.find(|c: char| c == ')' || c == '(' || c == ' ' || c.is_ascii_control());
            if let Some(offset) = offset {
                if offset != 0 {
                    f.write_str(&s[..offset])?;
                }
                // matched characters are ascii so this should be true.
                debug_assert!(s.is_char_boundary(offset));
                debug_assert!(s.is_char_boundary(offset + 1));

                let c: char = s[offset..].chars().next().unwrap();
                use std::fmt::Write;
                if c == '(' || c == ')' {
                    f.write_char('\\')?;
                    f.write_str(&s[offset..offset + 1])?;
                } else {
                    write!(f, "%{:02X}", u32::from(c))?;
                }
                s = &s[offset + 1..];
            } else {
                f.write_str(s)?;
                break;
            }
        }
        Ok(())
    }
}

fn main() {
    simple_logger::init_with_level(log::Level::Trace).unwrap();
    log::set_max_level(log::LevelFilter::Warn);
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

    let mut multi = MultiSubreddit::new(&app, username, &["programming", "rust", "NixOS", "linux"]);
    loop {
        match multi.load_comments_made() {
            Ok(_) => break,
            Err(e) => {
                println!("Error in load_comments_made: {}", e);
                continue;
            }
        }
    }
    multi.process();
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

    #[test]
    fn test_escape_markdown_link() {
        let tests: &[(&'static str, &'static str)] = &[
            ("/test", "/test"),
            ("/ test", "/%20test"),
            ("/ ", "/%20"),
            ("/\n", "/%0A"),
            ("/(x)", "/\\(x\\)"),
        ];
        for (input, expect) in tests {
            assert_eq!(expect, &format!("{}", EscapeMarkdownLink(input)));
        }
    }
}
