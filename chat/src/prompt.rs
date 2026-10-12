//! Turning a conversation into a single cedar prompt.
//!
//! Normally a new message continues the previous cedar run, which carries the conversation. This
//! is the fallback for when that run no longer exists on cedar: the earlier exchanges this service
//! remembers are inlined as context, newest kept first when the history has to be cut to fit.

/// One earlier exchange: what the user wrote and the agent's final answer.
pub struct Exchange<'a> {
    pub user: &'a str,
    pub assistant: &'a str,
}

pub fn compose(history: &[Exchange<'_>], message: &str, max_history_bytes: usize) -> String {
    let mut kept: Vec<String> = Vec::new();
    let mut used = 0;
    for ex in history.iter().rev() {
        let block = format!("<user>\n{}\n</user>\n<assistant>\n{}\n</assistant>\n", ex.user, ex.assistant);
        if used + block.len() > max_history_bytes {
            break;
        }
        used += block.len();
        kept.push(block);
    }
    if kept.is_empty() && history.is_empty() {
        return message.to_string();
    }
    kept.reverse();

    let mut out = String::from(
        "This message continues an earlier conversation, shown below for context. Your sandbox has \
         been reset since then, so files and processes from earlier exchanges no longer exist.\n\n\
         <conversation_history>\n",
    );
    let omitted = history.len() - kept.len();
    if omitted > 0 {
        out.push_str(&format!("({omitted} earlier exchange(s) omitted)\n"));
    }
    for block in kept {
        out.push_str(&block);
    }
    out.push_str("</conversation_history>\n\nReply to this new message:\n\n");
    out.push_str(message);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_message_is_sent_as_is() {
        assert_eq!(compose(&[], "hello", 1000), "hello");
    }

    #[test]
    fn history_is_inlined_oldest_first() {
        let h = [Exchange { user: "q1", assistant: "a1" }, Exchange { user: "q2", assistant: "a2" }];
        let p = compose(&h, "q3", 10_000);
        let (i1, i2, i3) = (p.find("q1").unwrap(), p.find("a2").unwrap(), p.rfind("q3").unwrap());
        assert!(i1 < i2 && i2 < i3);
        assert!(p.ends_with("q3"));
        assert!(!p.contains("omitted"));
    }

    #[test]
    fn oldest_exchanges_are_dropped_to_fit() {
        let long = "x".repeat(500);
        let h = [Exchange { user: "old", assistant: &long }, Exchange { user: "new", assistant: "short" }];
        let p = compose(&h, "now", 200);
        assert!(!p.contains("old"));
        assert!(p.contains("new"));
        assert!(p.contains("1 earlier exchange(s) omitted"));
    }
}
