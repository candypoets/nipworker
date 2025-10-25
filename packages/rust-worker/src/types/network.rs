use rustc_hash::FxHashMap;

use crate::types::nostr::Filter;

use crate::generated::nostr::fb;

#[derive(Default)]
pub struct Request {
    pub ids: Vec<String>,

    pub authors: Vec<String>,

    pub kinds: Vec<i32>,

    pub tags: FxHashMap<String, Vec<String>>,

    pub since: Option<i32>,

    pub until: Option<i32>,

    pub limit: Option<i32>,

    pub search: Option<String>,

    pub relays: Vec<String>,

    pub close_on_eose: bool,

    pub cache_first: bool,

    pub no_cache: bool,

    pub max_relays: u32,
}

impl Request {
    pub fn from_flatbuffer(fb_req: &fb::Request) -> Self {
        let mut tags = FxHashMap::default();
        if let Some(fb_tags) = fb_req.tags() {
            for i in 0..fb_tags.len() {
                let tag_vec = fb_tags.get(i);
                if let Some(items) = tag_vec.items() {
                    if items.len() >= 2 {
                        let key = items.get(0).to_string();
                        let values: Vec<String> =
                            (1..items.len()).map(|j| items.get(j).to_string()).collect();
                        tags.insert(key, values);
                    }
                }
            }
        }

        Request {
            ids: fb_req
                .ids()
                .map(|v| v.iter().map(|s| s.to_string()).collect())
                .unwrap_or_default(),
            authors: fb_req
                .authors()
                .map(|v| v.iter().map(|s| s.to_string()).collect())
                .unwrap_or_default(),
            kinds: fb_req
                .kinds()
                .map(|v| v.iter().map(|k| k as i32).collect())
                .unwrap_or_default(),
            tags,
            since: if fb_req.since() != 0 {
                Some(fb_req.since())
            } else {
                None
            },
            until: if fb_req.until() != 0 {
                Some(fb_req.until())
            } else {
                None
            },
            limit: if fb_req.limit() != 0 {
                Some(fb_req.limit())
            } else {
                None
            },
            search: if fb_req.search().is_some() {
                Some(fb_req.search().unwrap().to_string())
            } else {
                None
            },
            relays: fb_req
                .relays()
                .map(|v| v.iter().map(|s| s.to_string()).collect())
                .unwrap_or_default(),
            close_on_eose: fb_req.close_on_eose(),
            cache_first: fb_req.cache_first(),
            no_cache: fb_req.no_cache(),
            max_relays: fb_req.max_relays() as u32,
        }
    }

    pub fn build_flatbuffer<'a>(
        &self,
        fbb: &mut flatbuffers::FlatBufferBuilder<'a>,
    ) -> flatbuffers::WIPOffset<fb::Request<'a>> {
        // Vec<String> -> Flatbuffer string vector
        let ids = if !self.ids.is_empty() {
            let strs: Vec<_> = self.ids.iter().map(|s| fbb.create_string(s)).collect();
            Some(fbb.create_vector(&strs))
        } else {
            None
        };

        let authors = if !self.authors.is_empty() {
            let strs: Vec<_> = self.authors.iter().map(|s| fbb.create_string(s)).collect();
            Some(fbb.create_vector(&strs))
        } else {
            None
        };

        let relays = if !self.relays.is_empty() {
            let strs: Vec<_> = self.relays.iter().map(|s| fbb.create_string(s)).collect();
            Some(fbb.create_vector(&strs))
        } else {
            None
        };

        let kinds = if !self.kinds.is_empty() {
            let vals: Vec<u16> = self.kinds.iter().map(|k| *k as u16).collect();
            Some(fbb.create_vector(&vals))
        } else {
            None
        };

        let search = if let Some(ref search) = self.search {
            Some(fbb.create_string(search))
        } else {
            None
        };

        // tags are just [StringVec] where StringVec { items: [string]; }
        let tags = if !self.tags.is_empty() {
            let mut tag_offsets = Vec::new();
            for (k, vs) in &self.tags {
                // Put the key as the first element, followed by values
                let mut items_vec = Vec::with_capacity(1 + vs.len());
                items_vec.push(fbb.create_string(k));
                for v in vs {
                    items_vec.push(fbb.create_string(v));
                }
                let fb_items = fbb.create_vector(&items_vec);
                let sv = fb::StringVec::create(
                    fbb,
                    &fb::StringVecArgs {
                        items: Some(fb_items),
                    },
                );
                tag_offsets.push(sv);
            }
            Some(fbb.create_vector(&tag_offsets))
        } else {
            None
        };

        fb::Request::create(
            fbb,
            &fb::RequestArgs {
                ids,
                authors,
                kinds,
                tags,
                limit: self.limit.unwrap_or_default(),
                since: self.since.unwrap_or_default(),
                until: self.until.unwrap_or_default(),
                search,
                relays,
                close_on_eose: self.close_on_eose,
                cache_first: self.cache_first,
                no_cache: self.no_cache,
                max_relays: self.max_relays as u16,
            },
        )
    }

    // pub fn new(relays: Vec<String>, filter: Filter) -> Self {
    //     Self {
    //         ids: filter
    //             .ids
    //             .map(|ids| ids.into_iter().map(|id| id.to_hex()).collect())
    //             .unwrap_or_default(),
    //         authors: filter
    //             .authors
    //             .map(|authors| authors.into_iter().map(|pk| pk.to_hex()).collect())
    //             .unwrap_or_default(),
    //         kinds: filter
    //             .kinds
    //             .map(|kinds| kinds.into_iter().map(|k| k as i32).collect())
    //             .unwrap_or_default(),
    //         tags: FxHashMap::default(), // TODO: Convert filter tags properly
    //         since: filter.since.map(|ts| ts as i32),
    //         until: filter.until.map(|ts| ts as i32),
    //         limit: filter.limit.map(|l| l as i32),
    //         search: filter.search,
    //         relays,
    //         close_on_eose: false,
    //         cache_first: false,
    //     }
    // }

    pub fn to_filter(&self) -> Result<Filter, crate::types::TypesError> {
        let mut filter = Filter::new();

        if !self.ids.is_empty() {
            filter.ids = Some(
                self.ids
                    .iter()
                    .map(|id| crate::types::nostr::EventId::from_hex(id))
                    .collect::<Result<Vec<_>, _>>()?,
            );
        }

        if !self.authors.is_empty() {
            filter.authors = Some(
                self.authors
                    .iter()
                    .map(|author| crate::types::nostr::PublicKey::from_hex(author))
                    .collect::<Result<Vec<_>, _>>()?,
            );
        }

        if !self.kinds.is_empty() {
            filter.kinds = Some(self.kinds.iter().map(|k| *k as u16).collect());
        }

        if let Some(since) = self.since {
            filter.since = Some(since as u64);
        }

        if let Some(until) = self.until {
            filter.until = Some(until as u64);
        }

        if let Some(limit) = self.limit {
            filter.limit = Some(limit as usize);
        }

        if let Some(ref search) = self.search {
            filter.search = Some(search.clone());
        }

        // Convert tags from HashMap to the proper filter format
        if !self.tags.is_empty() {
            for (key, values) in &self.tags {
                match key.as_str() {
                    "#e" => filter.e_tags = Some(values.clone()),
                    "#p" => filter.p_tags = Some(values.clone()),
                    "#d" => filter.d_tags = Some(values.clone()),
                    "#a" => filter.a_tags = Some(values.clone()),
                    _ => {
                        filter
                            .tags
                            .get_or_insert_with(std::collections::HashMap::new)
                            .insert(
                                key.strip_prefix('#').unwrap_or(key).to_string(),
                                values.clone(),
                            );
                        // Handle other tag types if needed
                        // For now, we'll ignore unknown tag types
                    }
                }
            }
        }

        Ok(filter)
    }
}
