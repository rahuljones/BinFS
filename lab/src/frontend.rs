use std::{
    cmp::Ordering,
    collections::HashSet,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use tonic::async_trait;
use tribbler::{
    err::{TribResult, TribblerError},
    storage::{BinStorage, KeyValue, Storage},
    trib::{
        is_valid_username, Server, Trib, MAX_FOLLOWING, MAX_TRIB_FETCH, MAX_TRIB_LEN, MIN_LIST_USER,
    },
};

#[derive(Serialize, Deserialize, Debug, Clone)]
struct FollowEntry {
    clock: u64,
    follow: bool,
    target: String,
}

pub struct FrontEnd {
    pub bin_storage: Box<dyn BinStorage>,
}

impl FrontEnd {
    async fn get_user_bin(&self, user: &str) -> TribResult<Box<dyn Storage>> {
        if !is_valid_username(user) {
            return Err(TribblerError::InvalidUsername(user.to_string()).into());
        }

        let bin = self.bin_storage.bin(user).await?;
        if bin.get("signed_up").await?.is_none() {
            return Err(TribblerError::UserDoesNotExist(user.to_string()).into());
        }

        Ok(bin)
    }

    async fn parse_following(
        &self,
        user_bin: &Box<dyn Storage>,
    ) -> TribResult<(Vec<FollowEntry>, HashSet<String>)> {
        let follow_log_raw = user_bin.list_get("follow_log").await?.0;
        let mut follow_log: Vec<FollowEntry> = follow_log_raw
            .iter()
            .filter_map(|e| serde_json::from_str(e).ok())
            .collect();

        follow_log.sort_by(|a, b| a.clock.cmp(&b.clock).then(a.target.cmp(&b.target)));

        let mut current_following: HashSet<String> = HashSet::new();
        for follow_log_entry in &follow_log {
            if follow_log_entry.follow {
                // follow target
                current_following.insert(follow_log_entry.target.clone());
            } else {
                // unfollow target
                current_following.remove(&follow_log_entry.target);
            }
        }
        Ok((follow_log, current_following))
    }
}

#[async_trait]
impl Server for FrontEnd {
    /// Creates a user.
    ///
    /// - Returns error when the username is invalid;
    /// - Returns error when the user already exists.
    /// - Concurrent sign ups on the same user might both succeed with no error.
    async fn sign_up(&self, user: &str) -> TribResult<()> {
        if !is_valid_username(user) {
            return Err(TribblerError::InvalidUsername(user.to_string()).into());
        }

        let user_bin = self.bin_storage.bin(user).await?;
        if user_bin.get("signed_up").await?.is_some() {
            return Err(TribblerError::UsernameTaken(user.to_string()).into());
        }

        // TODO: might need to remove duplicates
        user_bin
            .set(&KeyValue {
                key: "signed_up".to_string(),
                value: "true".to_string(),
            })
            .await?;

        let users_bin = self.bin_storage.bin("__users__").await?;
        let current_users = users_bin.list_get("names").await?.0;

        // this is the users cache, once its above MIN_LIST_USER, we don't need to add any more
        if current_users.len() < MIN_LIST_USER {
            if !current_users.contains(&user.to_string()) {
                users_bin
                    .list_append(&KeyValue {
                        key: "names".to_string(),
                        value: user.to_string(),
                    })
                    .await?;
            }
        }

        Ok(())
    }

    /// List 20 registered users.
    ///
    /// - When there are less than 20 users that signed up the service, all of
    /// them need to be listed.
    /// - When there are more than 20 users that signed up the service, an
    /// arbitrary set of at lest 20 of them needs to be listed.
    /// - The result should be sorted in alphabetical order.
    async fn list_users(&self) -> TribResult<Vec<String>> {
        let users_bin = self.bin_storage.bin("__users__").await?;
        let list = users_bin.list_get("names").await?;
        let mut users: Vec<String> = list.0.iter().cloned().collect();
        users.sort();
        users.dedup();
        users.truncate(MIN_LIST_USER);
        Ok(users)
    }

    /// Post a tribble. The clock is the maximum clock value this user has seen
    /// so far by reading tribbles or clock sync.
    ///
    /// - Returns error when who does not exist;
    /// - Returns error when post is too long.
    async fn post(&self, who: &str, post: &str, clock: u64) -> TribResult<()> {
        let user_bin = self.get_user_bin(who).await?;

        if post.len() > MAX_TRIB_LEN {
            return Err(TribblerError::TribTooLong.into());
        }

        let machine_time = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
        let new_clock = user_bin.clock(clock + 1).await?;

        let trib = Trib {
            user: who.to_string(),
            message: post.to_string(),
            time: machine_time as u64,
            clock: new_clock,
        };

        user_bin
            .list_append(&KeyValue {
                key: "tribs".to_string(),
                value: serde_json::to_string(&trib)?,
            })
            .await?;

        // garbage collection
        let tribs_list = user_bin.list_get("tribs").await?.0;
        if tribs_list.len() > MAX_TRIB_FETCH + 10 {
            let mut decoded_tribs: Vec<Trib> = tribs_list
                .iter()
                .filter_map(|t| serde_json::from_str(t).ok())
                .collect();

            decoded_tribs.sort_by(trib_order);
            let to_remove_count = decoded_tribs.len().saturating_sub(MAX_TRIB_FETCH);

            for i in 0..to_remove_count {
                let encoded_old_trib = serde_json::to_string(&decoded_tribs[i])?;
                user_bin
                    .list_remove(&KeyValue {
                        key: "tribs".to_string(),
                        value: encoded_old_trib,
                    })
                    .await?;
            }
        }

        Ok(())
    }

    /// List the tribs that a particular user posted.
    /// Returns error when user has not signed up.
    async fn tribs(&self, user: &str) -> TribResult<Vec<Arc<Trib>>> {
        let user_bin = self.get_user_bin(user).await?;
        let mut user_tribs: Vec<Trib> = user_bin
            .list_get("tribs")
            .await?
            .0
            .iter()
            .filter_map(|trib_encoded| serde_json::from_str(trib_encoded).ok())
            .collect();
        user_tribs.sort_by(trib_order);
        let len = user_tribs.len();

        let output = user_tribs
            .into_iter()
            .skip(len.saturating_sub(MAX_TRIB_FETCH))
            .map(Arc::new)
            .collect();

        Ok(output)
    }

    /// Follow someone's timeline.
    ///
    /// - Returns error when who == whom;
    /// - Returns error when who is already following whom;
    /// - Returns error when who is trying to following more than
    ///   [MAX_FOLLOWING] users.
    /// - Returns error when who or whom has not signed up.
    ///
    /// Concurrent follows might both succeed without error. The count of
    /// following users might exceed [MAX_FOLLOWING]=2000, if and only if the
    /// 2000'th user is generated by concurrent Follow() calls.
    async fn follow(&self, who: &str, whom: &str) -> TribResult<()> {
        if who == whom {
            return Err(TribblerError::WhoWhom(who.to_string()).into());
        }

        let who_bin = self.get_user_bin(who).await?;
        let _ = self.get_user_bin(whom).await?;

        // quick + early check. this will be fine for any scenario where we don't have concurrent follows
        let (_, following_set) = self.parse_following(&who_bin).await?;
        if following_set.contains(&whom.to_string()) {
            return Err(TribblerError::AlreadyFollowing(who.to_string(), whom.to_string()).into());
        }

        if following_set.len() >= MAX_FOLLOWING {
            return Err(TribblerError::FollowingTooMany.into());
        }

        let clock = who_bin.clock(0).await?;
        let follow_entry = FollowEntry {
            clock,
            follow: true,
            target: whom.to_string(),
        };
        who_bin
            .list_append(&KeyValue {
                key: "follow_log".to_string(),
                value: serde_json::to_string(&follow_entry)?,
            })
            .await?;

        let (follow_log, _) = self.parse_following(&who_bin).await?;
        let mut current_state = false;
        let mut last_flip_to_following: Option<u64> = None;
        for follow_entry in &follow_log {
            if follow_entry.target != whom {
                continue;
            }

            if follow_entry.follow && !current_state {
                // current entry is follow and last entry was unfollow - this is the first follow
                current_state = true;
                last_flip_to_following = Some(follow_entry.clock);
            } else if !follow_entry.follow && current_state {
                // current entry is unfollow and last entry was follow - this is an unfollow
                current_state = false;
            }
        }

        if current_state && last_flip_to_following == Some(clock) {
            Ok(())
        } else {
            Err(TribblerError::AlreadyFollowing(who.to_string(), whom.to_string()).into())
        }
    }

    /// Unfollow someone's timeline.
    ///
    /// - Returns error when who == whom.
    /// - Returns error when who is not following whom;
    /// - Returns error when who or whom has not signed up.
    async fn unfollow(&self, who: &str, whom: &str) -> TribResult<()> {
        if who == whom {
            return Err(TribblerError::WhoWhom(who.to_string()).into());
        }

        let who_bin = self.get_user_bin(who).await?;
        let _ = self.get_user_bin(whom).await?;

        let (_, following_set) = self.parse_following(&who_bin).await?;
        if !following_set.contains(whom) {
            return Err(TribblerError::NotFollowing(who.to_string(), whom.to_string()).into());
        }

        let clock = who_bin.clock(0).await?;
        let follow_entry = FollowEntry {
            clock: clock,
            follow: false,
            target: whom.to_string(),
        };
        who_bin
            .list_append(&KeyValue {
                key: "follow_log".to_string(),
                value: serde_json::to_string(&follow_entry)?,
            })
            .await?;

        let (follow_log, _) = self.parse_following(&who_bin).await?;
        let mut current_state = false;
        let mut last_flip_to_not_following: Option<u64> = None;
        for follow_entry in &follow_log {
            if follow_entry.target != whom {
                continue;
            }

            if follow_entry.follow && !current_state {
                current_state = true;
            } else if !follow_entry.follow && current_state {
                current_state = false;
                last_flip_to_not_following = Some(follow_entry.clock);
            }
        }

        if !current_state && last_flip_to_not_following == Some(clock) {
            Ok(())
        } else {
            Err(TribblerError::NotFollowing(who.to_string(), whom.to_string()).into())
        }
    }

    /// Checks if `who` is following `whom`
    ///
    /// - Returns true when who following whom.
    /// - Returns error when who == whom.
    /// - Returns error when who or whom has not signed up.
    async fn is_following(&self, who: &str, whom: &str) -> TribResult<bool> {
        if who == whom {
            return Err(TribblerError::WhoWhom(who.to_string()).into());
        }

        let who_bin = self.get_user_bin(who).await?;
        let _ = self.get_user_bin(whom).await?;

        let (_, following_set) = self.parse_following(&who_bin).await?;
        Ok(following_set.contains(&whom.to_string()))
    }

    /// Gets the list of users `who` is following
    ///
    /// - Returns the list of following users.
    /// - Returns error when who has not signed up.
    ///
    /// The list may have more users more than [MAX_FOLLOWING]=2000,
    /// if and only if the 2000'th user is generate d by concurrent Follow()
    /// calls.
    async fn following(&self, who: &str) -> TribResult<Vec<String>> {
        let who_bin = self.get_user_bin(who).await?;
        let (_, following_set) = self.parse_following(&who_bin).await?;

        Ok(following_set.into_iter().collect())
    }

    /// List the tribs of someone's following users (including himself).
    ///
    /// - Returns error when user has not signed up.
    async fn home(&self, user: &str) -> TribResult<Vec<Arc<Trib>>> {
        let mut following = self.following(user).await?;
        let mut tribs: Vec<Arc<Trib>> = Vec::new();

        following.push(user.to_string());
        for username in &following {
            let mut user_tribs = self.tribs(username).await?;
            tribs.append(&mut user_tribs);
        }

        tribs.sort_by(|a, b| trib_order(a, b));
        let len = tribs.len();

        Ok(tribs
            .into_iter()
            .skip(len.saturating_sub(MAX_TRIB_FETCH))
            .collect())
    }
}

fn trib_order(a: &Trib, b: &Trib) -> Ordering {
    a.clock
        .cmp(&b.clock)
        .then(a.time.cmp(&b.time))
        .then(a.user.cmp(&b.user))
        .then(a.message.cmp(&b.message))
}
