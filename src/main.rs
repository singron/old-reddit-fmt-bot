extern crate comrak;
extern crate htmlescape;
extern crate orca;

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

fn main() {
    let secret = get_pass("Reddit/old-reddit-fmt-bot/secret");
    let id = get_pass("Reddit/old-reddit-fmt-bot/id");
    let password = get_pass("Misc/reddit.com/old-reddit-fmt-bot");
    let mut app = orca::App::new("old fmt experiment", "0.1.0", "singron").unwrap();
    let username = "old-reddit-fmt-bot";
    app.authorize_script(&id, &secret, username, &password)
        .unwrap();
    let comments: orca::data::Comments = app.create_comment_stream("nixos");
    for comment in comments {
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
        if contains_fenced_block(&body) {
            // This comment from the comments stream doesn't include replies, so let's load the
            // whole tree.
            let tree = match app.get_comment_tree(strip_type(&comment.link_id)) {
                Err(e) => {
                    println!("Error: {:?}", e);
                    continue;
                }
                Ok(x) => x,
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
            println!(
                "https://www.reddit.com/r/{}/comments/{}/_/{}",
                comment.subreddit,
                strip_type(&comment.link_id),
                comment.id
            );
            println!("{}", body);
            let reply = "Your comment uses one or more fenced code blocks. These don't render correctly in old reddit even if you authored them in new reddit. Please use code blocks indented with 4 spaces instead. See [my page](https://github.com/singron/old-reddit-fmt-bot/blob/master/about.md) for easy ways to do this and for information about this bot.";
            println!("{}", reply);
        }
    }
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
