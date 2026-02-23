//! Utilities to reconstruct a WorkerMessage carrying either a ParsedEvent or a NostrEvent,
//! injecting a subscription id for cache_response ring publication.
//!
//! This module rebuilds ParsedEvent including:
//! - parsed union (all kinds declared in schemas/kinds/*)
//! - requests[]
//! - relays[]
//! - tags[]
//! and recursively handles Kind6Parsed.reposted_event.

use flatbuffers::{FlatBufferBuilder, ForwardsUOffset, Vector, WIPOffset};
use shared::generated::nostr::fb;
use tracing::info;

//
// Generic helpers
//

fn build_string_vector<'a>(
    builder: &mut FlatBufferBuilder<'a>,
    view: Vector<'_, ForwardsUOffset<&str>>,
) -> WIPOffset<Vector<'a, ForwardsUOffset<&'a str>>> {
    let mut items: Vec<WIPOffset<&str>> = Vec::with_capacity(view.len());
    for i in 0..view.len() {
        items.push(builder.create_string(view.get(i)));
    }
    builder.create_vector(&items)
}

fn build_tags_vector<'a>(
    builder: &mut FlatBufferBuilder<'a>,
    tags_view: Vector<'_, ForwardsUOffset<fb::StringVec<'_>>>,
) -> WIPOffset<Vector<'a, ForwardsUOffset<fb::StringVec<'a>>>> {
    let mut tag_offsets: Vec<WIPOffset<fb::StringVec<'_>>> = Vec::with_capacity(tags_view.len());

    for i in 0..tags_view.len() {
        let sv = tags_view.get(i);
        let items_off = if let Some(items) = sv.items() {
            let mut item_strs: Vec<WIPOffset<&str>> = Vec::with_capacity(items.len());
            for j in 0..items.len() {
                item_strs.push(builder.create_string(items.get(j)));
            }
            Some(builder.create_vector(&item_strs))
        } else {
            None
        };

        let sv_off = fb::StringVec::create(builder, &fb::StringVecArgs { items: items_off });
        tag_offsets.push(sv_off);
    }

    builder.create_vector(&tag_offsets)
}

//
// Common types builders (from common.fbs)
//

fn build_profile_pointer<'a>(
    builder: &mut FlatBufferBuilder<'a>,
    pp: fb::ProfilePointer<'_>,
) -> WIPOffset<fb::ProfilePointer<'a>> {
    let public_key = builder.create_string(pp.public_key());
    let relays = pp.relays().map(|rv| build_string_vector(builder, rv));
    fb::ProfilePointer::create(
        builder,
        &fb::ProfilePointerArgs {
            public_key: Some(public_key),
            relays,
        },
    )
}

fn build_event_pointer<'a>(
    builder: &mut FlatBufferBuilder<'a>,
    ep: fb::EventPointer<'_>,
) -> WIPOffset<fb::EventPointer<'a>> {
    let id = builder.create_string(ep.id());
    let relays = ep.relays().map(|rv| build_string_vector(builder, rv));
    let author = ep.author().map(|a| builder.create_string(a));
    fb::EventPointer::create(
        builder,
        &fb::EventPointerArgs {
            id: Some(id),
            relays,
            author,
            kind: ep.kind(),
        },
    )
}

fn build_emoji<'a>(
    builder: &mut FlatBufferBuilder<'a>,
    e: fb::Emoji<'_>,
) -> WIPOffset<fb::Emoji<'a>> {
    let shortcode = builder.create_string(e.shortcode());
    let url = builder.create_string(e.url());
    fb::Emoji::create(
        builder,
        &fb::EmojiArgs {
            shortcode: Some(shortcode),
            url: Some(url),
        },
    )
}

fn build_contact_vector<'a>(
    builder: &mut FlatBufferBuilder<'a>,
    contacts: Vector<'_, ForwardsUOffset<fb::Contact<'_>>>,
) -> WIPOffset<Vector<'a, ForwardsUOffset<fb::Contact<'a>>>> {
    let mut offs = Vec::with_capacity(contacts.len());
    for i in 0..contacts.len() {
        let c = contacts.get(i);
        let pubkey = builder.create_string(c.pubkey());
        let relays = c.relays().map(|rv| build_string_vector(builder, rv));
        let petname = c.petname().map(|p| builder.create_string(p));
        offs.push(fb::Contact::create(
            builder,
            &fb::ContactArgs {
                pubkey: Some(pubkey),
                relays,
                petname,
            },
        ));
    }
    builder.create_vector(&offs)
}

fn build_relay_info_vector<'a>(
    builder: &mut FlatBufferBuilder<'a>,
    relays: Vector<'_, ForwardsUOffset<fb::RelayInfo<'_>>>,
) -> WIPOffset<Vector<'a, ForwardsUOffset<fb::RelayInfo<'a>>>> {
    let mut offs = Vec::with_capacity(relays.len());
    for i in 0..relays.len() {
        let r = relays.get(i);
        let url = builder.create_string(r.url());
        offs.push(fb::RelayInfo::create(
            builder,
            &fb::RelayInfoArgs {
                url: Some(url),
                read: r.read(),
                write: r.write(),
            },
        ));
    }
    builder.create_vector(&offs)
}

fn build_mint_info_vector<'a>(
    builder: &mut FlatBufferBuilder<'a>,
    mints: Vector<'_, ForwardsUOffset<fb::MintInfo<'_>>>,
) -> WIPOffset<Vector<'a, ForwardsUOffset<fb::MintInfo<'a>>>> {
    let mut offs = Vec::with_capacity(mints.len());
    for i in 0..mints.len() {
        let m = mints.get(i);
        let url = builder.create_string(m.url());
        let base = Some(build_string_vector(builder, m.base_units()));
        offs.push(fb::MintInfo::create(
            builder,
            &fb::MintInfoArgs {
                url: Some(url),
                base_units: base,
            },
        ));
    }
    builder.create_vector(&offs)
}

fn build_dleq_proof<'a>(
    builder: &mut FlatBufferBuilder<'a>,
    d: fb::DLEQProof<'_>,
) -> WIPOffset<fb::DLEQProof<'a>> {
    let e = builder.create_string(d.e());
    let s = builder.create_string(d.s());
    let r = builder.create_string(d.r());
    fb::DLEQProof::create(
        builder,
        &fb::DLEQProofArgs {
            e: Some(e),
            s: Some(s),
            r: Some(r),
        },
    )
}

fn build_p2pk_witness<'a>(
    builder: &mut FlatBufferBuilder<'a>,
    w: fb::P2PKWitness<'_>,
) -> WIPOffset<fb::P2PKWitness<'a>> {
    let sigs = w.signatures().map(|rv| build_string_vector(builder, rv));
    fb::P2PKWitness::create(builder, &fb::P2PKWitnessArgs { signatures: sigs })
}

fn build_htlc_witness<'a>(
    builder: &mut FlatBufferBuilder<'a>,
    w: fb::HTLCWitness<'_>,
) -> WIPOffset<fb::HTLCWitness<'a>> {
    let preimage = builder.create_string(w.preimage());
    let sigs = w.signatures().map(|rv| build_string_vector(builder, rv));
    fb::HTLCWitness::create(
        builder,
        &fb::HTLCWitnessArgs {
            preimage: Some(preimage),
            signatures: sigs,
        },
    )
}

fn build_proof_vector<'a>(
    builder: &mut FlatBufferBuilder<'a>,
    proofs: Vector<'_, ForwardsUOffset<fb::Proof<'_>>>,
) -> WIPOffset<Vector<'a, ForwardsUOffset<fb::Proof<'a>>>> {
    let mut offs = Vec::with_capacity(proofs.len());
    for i in 0..proofs.len() {
        let p = proofs.get(i);
        let id = builder.create_string(p.id());
        let secret = builder.create_string(p.secret());
        let c = builder.create_string(p.c());
        let dleq = p.dleq().map(|d| build_dleq_proof(builder, d));

        // Build witness union using parent table methods
        let (witness_type, witness_val_opt) = match p.witness_type() {
            fb::Witness::WitnessString => {
                if let Some(ws) = p.witness_as_witness_string() {
                    let val = ws.value().map(|v| builder.create_string(v));
                    let off =
                        fb::WitnessString::create(builder, &fb::WitnessStringArgs { value: val });
                    (fb::Witness::WitnessString, Some(off.as_union_value()))
                } else {
                    (fb::Witness::NONE, None)
                }
            }
            fb::Witness::P2PKWitness => {
                if let Some(w) = p.witness_as_p2_pkwitness() {
                    let off = build_p2pk_witness(builder, w);
                    (fb::Witness::P2PKWitness, Some(off.as_union_value()))
                } else {
                    (fb::Witness::NONE, None)
                }
            }
            fb::Witness::HTLCWitness => {
                if let Some(w) = p.witness_as_htlcwitness() {
                    let off = build_htlc_witness(builder, w);
                    (fb::Witness::HTLCWitness, Some(off.as_union_value()))
                } else {
                    (fb::Witness::NONE, None)
                }
            }
            _ => (fb::Witness::NONE, None),
        };

        let off = fb::Proof::create(
            builder,
            &fb::ProofArgs {
                amount: p.amount(),
                id: Some(id),
                secret: Some(secret),
                c: Some(c),
                dleq,
                witness_type,
                witness: witness_val_opt,
                version: p.version(),
            },
        );
        offs.push(off);
    }
    builder.create_vector(&offs)
}

fn build_history_tag_vector<'a>(
    builder: &mut FlatBufferBuilder<'a>,
    tags: Vector<'_, ForwardsUOffset<fb::HistoryTag<'_>>>,
) -> WIPOffset<Vector<'a, ForwardsUOffset<fb::HistoryTag<'a>>>> {
    let mut offs = Vec::with_capacity(tags.len());
    for i in 0..tags.len() {
        let t = tags.get(i);
        let name = builder.create_string(t.name());
        let value = builder.create_string(t.value());
        let relay = t.relay().map(|s| builder.create_string(s));
        let marker = t.marker().map(|s| builder.create_string(s));
        offs.push(fb::HistoryTag::create(
            builder,
            &fb::HistoryTagArgs {
                name: Some(name),
                value: Some(value),
                relay,
                marker,
            },
        ));
    }
    builder.create_vector(&offs)
}

fn build_zap_request<'a>(
    builder: &mut FlatBufferBuilder<'a>,
    z: fb::ZapRequest<'_>,
) -> WIPOffset<fb::ZapRequest<'a>> {
    let pubkey = builder.create_string(z.pubkey());
    let content = builder.create_string(z.content());
    let tags = build_tags_vector(builder, z.tags());

    let signature = if let Some(sig) = z.signature() {
        Some(builder.create_string(sig))
    } else {
        None
    };

    fb::ZapRequest::create(
        builder,
        &fb::ZapRequestArgs {
            kind: z.kind(),
            pubkey: Some(pubkey),
            content: Some(content),
            tags: Some(tags),
            signature,
        },
    )
}

//
// ContentBlock + ContentData (for Kind1/Kind4)
//

fn build_image_data<'a>(
    builder: &mut FlatBufferBuilder<'a>,
    v: fb::ImageData<'_>,
) -> WIPOffset<fb::ImageData<'a>> {
    let url = builder.create_string(v.url());
    let alt = v.alt().map(|s| builder.create_string(s));
    let dim = v.dim().map(|s| builder.create_string(s));
    fb::ImageData::create(
        builder,
        &fb::ImageDataArgs {
            url: Some(url),
            alt,
            dim,
        },
    )
}

fn build_video_data<'a>(
    builder: &mut FlatBufferBuilder<'a>,
    v: fb::VideoData<'_>,
) -> WIPOffset<fb::VideoData<'a>> {
    let url = builder.create_string(v.url());
    let thumbnail = v.thumbnail().map(|s| builder.create_string(s));
    fb::VideoData::create(
        builder,
        &fb::VideoDataArgs {
            url: Some(url),
            thumbnail,
        },
    )
}

fn build_media_item<'a>(
    builder: &mut FlatBufferBuilder<'a>,
    mi: fb::MediaItem<'_>,
) -> WIPOffset<fb::MediaItem<'a>> {
    let image = mi.image().map(|img| build_image_data(builder, img));
    let video = mi.video().map(|v| build_video_data(builder, v));
    fb::MediaItem::create(builder, &fb::MediaItemArgs { image, video })
}

fn build_media_group_data<'a>(
    builder: &mut FlatBufferBuilder<'a>,
    mg: fb::MediaGroupData<'_>,
) -> WIPOffset<fb::MediaGroupData<'a>> {
    let items = mg.items().map(|iv| {
        let mut offs = Vec::with_capacity(iv.len());
        for i in 0..iv.len() {
            offs.push(build_media_item(builder, iv.get(i)));
        }
        builder.create_vector(&offs)
    });
    fb::MediaGroupData::create(builder, &fb::MediaGroupDataArgs { items })
}

fn build_content_block_vector<'a>(
    builder: &mut FlatBufferBuilder<'a>,
    blocks: Vector<'_, ForwardsUOffset<fb::ContentBlock<'_>>>,
) -> WIPOffset<Vector<'a, ForwardsUOffset<fb::ContentBlock<'a>>>> {
    let mut offs = Vec::with_capacity(blocks.len());
    for i in 0..blocks.len() {
        let b = blocks.get(i);
        let type_ = builder.create_string(b.type_());
        let text = builder.create_string(b.text());

        let (data_type, data_union_opt) = {
            let dt = b.data_type();
            match dt {
                fb::ContentData::CodeData => {
                    if let Some(v) = b.data_as_code_data() {
                        let language = v.language().map(|s| builder.create_string(s));
                        let code = builder.create_string(v.code());
                        let off = fb::CodeData::create(
                            builder,
                            &fb::CodeDataArgs {
                                language,
                                code: Some(code),
                            },
                        );
                        (fb::ContentData::CodeData, Some(off.as_union_value()))
                    } else {
                        (fb::ContentData::NONE, None)
                    }
                }
                fb::ContentData::HashtagData => {
                    if let Some(v) = b.data_as_hashtag_data() {
                        let tag = builder.create_string(v.tag());
                        let off = fb::HashtagData::create(
                            builder,
                            &fb::HashtagDataArgs { tag: Some(tag) },
                        );
                        (fb::ContentData::HashtagData, Some(off.as_union_value()))
                    } else {
                        (fb::ContentData::NONE, None)
                    }
                }
                fb::ContentData::CashuData => {
                    if let Some(v) = b.data_as_cashu_data() {
                        let token = builder.create_string(v.token());
                        let off = fb::CashuData::create(
                            builder,
                            &fb::CashuDataArgs { token: Some(token) },
                        );
                        (fb::ContentData::CashuData, Some(off.as_union_value()))
                    } else {
                        (fb::ContentData::NONE, None)
                    }
                }
                fb::ContentData::ImageData => {
                    if let Some(v) = b.data_as_image_data() {
                        let off = build_image_data(builder, v);
                        (fb::ContentData::ImageData, Some(off.as_union_value()))
                    } else {
                        (fb::ContentData::NONE, None)
                    }
                }
                fb::ContentData::VideoData => {
                    if let Some(v) = b.data_as_video_data() {
                        let off = build_video_data(builder, v);
                        (fb::ContentData::VideoData, Some(off.as_union_value()))
                    } else {
                        (fb::ContentData::NONE, None)
                    }
                }
                fb::ContentData::MediaGroupData => {
                    if let Some(v) = b.data_as_media_group_data() {
                        let off = build_media_group_data(builder, v);
                        (fb::ContentData::MediaGroupData, Some(off.as_union_value()))
                    } else {
                        (fb::ContentData::NONE, None)
                    }
                }
                fb::ContentData::NostrData => {
                    if let Some(v) = b.data_as_nostr_data() {
                        let id = builder.create_string(v.id());
                        let entity = builder.create_string(v.entity());
                        let relays = v.relays().map(|rv| build_string_vector(builder, rv));
                        let author = v.author().map(|s| builder.create_string(s));
                        let off = fb::NostrData::create(
                            builder,
                            &fb::NostrDataArgs {
                                id: Some(id),
                                entity: Some(entity),
                                relays,
                                author,
                                kind: v.kind(),
                            },
                        );
                        (fb::ContentData::NostrData, Some(off.as_union_value()))
                    } else {
                        (fb::ContentData::NONE, None)
                    }
                }
                fb::ContentData::LinkPreviewData => {
                    if let Some(v) = b.data_as_link_preview_data() {
                        let url = builder.create_string(v.url());
                        let title = v.title().map(|s| builder.create_string(s));
                        let description = v.description().map(|s| builder.create_string(s));
                        let image = v.image().map(|s| builder.create_string(s));
                        let off = fb::LinkPreviewData::create(
                            builder,
                            &fb::LinkPreviewDataArgs {
                                url: Some(url),
                                title,
                                description,
                                image,
                            },
                        );
                        (fb::ContentData::LinkPreviewData, Some(off.as_union_value()))
                    } else {
                        (fb::ContentData::NONE, None)
                    }
                }
                _ => (fb::ContentData::NONE, None),
            }
        };

        let off = fb::ContentBlock::create(
            builder,
            &fb::ContentBlockArgs {
                type_: Some(type_),
                text: Some(text),
                data_type,
                data: data_union_opt,
            },
        );
        offs.push(off);
    }
    builder.create_vector(&offs)
}

//
// Requests builder
//

fn build_requests_vector<'a>(
    builder: &mut FlatBufferBuilder<'a>,
    view: Vector<'_, ForwardsUOffset<fb::Request<'_>>>,
) -> WIPOffset<Vector<'a, ForwardsUOffset<fb::Request<'a>>>> {
    let mut req_offsets: Vec<WIPOffset<fb::Request<'_>>> = Vec::with_capacity(view.len());

    for i in 0..view.len() {
        let r = view.get(i);

        let ids_off = r.ids().map(|ids| {
            let mut v = Vec::with_capacity(ids.len());
            for j in 0..ids.len() {
                v.push(builder.create_string(ids.get(j)));
            }
            builder.create_vector(&v)
        });

        let authors_off = r.authors().map(|authors| {
            let mut v = Vec::with_capacity(authors.len());
            for j in 0..authors.len() {
                v.push(builder.create_string(authors.get(j)));
            }
            builder.create_vector(&v)
        });

        let kinds_off = r.kinds().map(|kinds| {
            let mut v: Vec<u16> = Vec::with_capacity(kinds.len());
            for k in kinds.into_iter() {
                v.push(k);
            }
            builder.create_vector(&v)
        });

        let tags_off = r.tags().map(|tags| build_tags_vector(builder, tags));

        let relays_off = r
            .relays()
            .map(|relays| build_string_vector(builder, relays));

        let search_off = r.search().map(|s| builder.create_string(s));

        let req_off = fb::Request::create(
            builder,
            &fb::RequestArgs {
                ids: ids_off,
                authors: authors_off,
                kinds: kinds_off,
                tags: tags_off,
                limit: r.limit(),
                since: r.since(),
                until: r.until(),
                search: search_off,
                relays: relays_off,
                close_on_eose: r.close_on_eose(),
                cache_first: r.cache_first(),
                no_cache: r.no_cache(),
                max_relays: r.max_relays(),
            },
        );

        req_offsets.push(req_off);
    }

    builder.create_vector(&req_offsets)
}

//
// ParsedData builders for all kinds
//

fn build_kind0<'a>(
    builder: &mut FlatBufferBuilder<'a>,
    v: fb::Kind0Parsed<'_>,
) -> WIPOffset<fb::Kind0Parsed<'a>> {
    let pubkey = v.pubkey().map(|s| builder.create_string(s));
    let name = v.name().map(|s| builder.create_string(s));
    let display_name = v.display_name().map(|s| builder.create_string(s));
    let picture = v.picture().map(|s| builder.create_string(s));
    let banner = v.banner().map(|s| builder.create_string(s));
    let about = v.about().map(|s| builder.create_string(s));
    let website = v.website().map(|s| builder.create_string(s));
    let nip05 = v.nip05().map(|s| builder.create_string(s));
    let lud06 = v.lud06().map(|s| builder.create_string(s));
    let lud16 = v.lud16().map(|s| builder.create_string(s));
    let github = v.github().map(|s| builder.create_string(s));
    let twitter = v.twitter().map(|s| builder.create_string(s));
    let mastodon = v.mastodon().map(|s| builder.create_string(s));
    let nostr = v.nostr().map(|s| builder.create_string(s));
    let display_name_alt = v.display_name_alt().map(|s| builder.create_string(s));
    let username = v.username().map(|s| builder.create_string(s));
    let bio = v.bio().map(|s| builder.create_string(s));
    let image = v.image().map(|s| builder.create_string(s));
    let avatar = v.avatar().map(|s| builder.create_string(s));
    let background = v.background().map(|s| builder.create_string(s));
    fb::Kind0Parsed::create(
        builder,
        &fb::Kind0ParsedArgs {
            pubkey,
            name,
            display_name,
            picture,
            banner,
            about,
            website,
            nip05,
            lud06,
            lud16,
            github,
            twitter,
            mastodon,
            nostr,
            display_name_alt,
            username,
            bio,
            image,
            avatar,
            background,
        },
    )
}

fn build_kind1<'a>(
    builder: &mut FlatBufferBuilder<'a>,
    v: fb::Kind1Parsed<'_>,
) -> WIPOffset<fb::Kind1Parsed<'a>> {
    let parsed_content = build_content_block_vector(builder, v.parsed_content());
    let shortened_content = v
        .shortened_content()
        .map(|sv| build_content_block_vector(builder, sv));
    let quotes = v.quotes().map(|pv| {
        let mut offs = Vec::with_capacity(pv.len());
        for i in 0..pv.len() {
            offs.push(build_profile_pointer(builder, pv.get(i)));
        }
        builder.create_vector(&offs)
    });
    let mentions = v.mentions().map(|ev| {
        let mut offs = Vec::with_capacity(ev.len());
        for i in 0..ev.len() {
            offs.push(build_event_pointer(builder, ev.get(i)));
        }
        builder.create_vector(&offs)
    });
    let reply = v.reply().map(|r| build_event_pointer(builder, r));
    let root = v.root().map(|r| build_event_pointer(builder, r));

    fb::Kind1Parsed::create(
        builder,
        &fb::Kind1ParsedArgs {
            parsed_content: Some(parsed_content),
            shortened_content,
            quotes,
            mentions,
            reply,
            root,
        },
    )
}

fn build_kind3<'a>(
    builder: &mut FlatBufferBuilder<'a>,
    v: fb::Kind3Parsed<'_>,
) -> WIPOffset<fb::Kind3Parsed<'a>> {
    let contacts = build_contact_vector(builder, v.contacts());
    fb::Kind3Parsed::create(
        builder,
        &fb::Kind3ParsedArgs {
            contacts: Some(contacts),
        },
    )
}

fn build_kind4<'a>(
    builder: &mut FlatBufferBuilder<'a>,
    v: fb::Kind4Parsed<'_>,
) -> WIPOffset<fb::Kind4Parsed<'a>> {
    let parsed_content = v
        .parsed_content()
        .map(|pc| build_content_block_vector(builder, pc));
    let decrypted_content = v.decrypted_content().map(|s| builder.create_string(s));
    let chat_id = builder.create_string(v.chat_id());
    let recipient = builder.create_string(v.recipient());
    fb::Kind4Parsed::create(
        builder,
        &fb::Kind4ParsedArgs {
            parsed_content,
            decrypted_content,
            chat_id: Some(chat_id),
            recipient: Some(recipient),
        },
    )
}

fn build_kind6<'a>(
    builder: &mut FlatBufferBuilder<'a>,
    v: fb::Kind6Parsed<'_>,
) -> WIPOffset<fb::Kind6Parsed<'a>> {
    // Recursively rebuild reposted ParsedEvent if present
    let reposted_event = v
        .reposted_event()
        .map(|re| rebuild_parsed_event(builder, re));
    fb::Kind6Parsed::create(builder, &fb::Kind6ParsedArgs { reposted_event })
}

fn build_kind7<'a>(
    builder: &mut FlatBufferBuilder<'a>,
    v: fb::Kind7Parsed<'_>,
) -> WIPOffset<fb::Kind7Parsed<'a>> {
    let event_id = builder.create_string(v.event_id());
    let pubkey = builder.create_string(v.pubkey());
    let emoji = v.emoji().map(|e| build_emoji(builder, e));
    let target_coordinates = v.target_coordinates().map(|s| builder.create_string(s));
    fb::Kind7Parsed::create(
        builder,
        &fb::Kind7ParsedArgs {
            reaction_type: v.reaction_type(),
            event_id: Some(event_id),
            pubkey: Some(pubkey),
            event_kind: v.event_kind(),
            emoji,
            target_coordinates,
        },
    )
}

fn build_kind17<'a>(
    builder: &mut FlatBufferBuilder<'a>,
    v: fb::Kind17Parsed<'_>,
) -> WIPOffset<fb::Kind17Parsed<'a>> {
    let event_id = builder.create_string(v.event_id());
    let pubkey = builder.create_string(v.pubkey());
    let emoji = v.emoji().map(|e| build_emoji(builder, e));
    let target_coordinates = v.target_coordinates().map(|s| builder.create_string(s));
    fb::Kind17Parsed::create(
        builder,
        &fb::Kind17ParsedArgs {
            reaction_type: v.reaction_type(),
            event_id: Some(event_id),
            pubkey: Some(pubkey),
            event_kind: v.event_kind(),
            emoji,
            target_coordinates,
        },
    )
}

fn build_kind10002<'a>(
    builder: &mut FlatBufferBuilder<'a>,
    v: fb::Kind10002Parsed<'_>,
) -> WIPOffset<fb::Kind10002Parsed<'a>> {
    let relays = build_relay_info_vector(builder, v.relays());
    fb::Kind10002Parsed::create(
        builder,
        &fb::Kind10002ParsedArgs {
            relays: Some(relays),
        },
    )
}

fn build_kind10019<'a>(
    builder: &mut FlatBufferBuilder<'a>,
    v: fb::Kind10019Parsed<'_>,
) -> WIPOffset<fb::Kind10019Parsed<'a>> {
    let trusted_mints = v
        .trusted_mints()
        .map(|mv| build_mint_info_vector(builder, mv));
    let p2pk_pubkey = v.p2pk_pubkey().map(|s| builder.create_string(s));
    let read_relays = v.read_relays().map(|rv| build_string_vector(builder, rv));
    fb::Kind10019Parsed::create(
        builder,
        &fb::Kind10019ParsedArgs {
            trusted_mints,
            p2pk_pubkey,
            read_relays,
        },
    )
}

fn build_kind17375<'a>(
    builder: &mut FlatBufferBuilder<'a>,
    v: fb::Kind17375Parsed<'_>,
) -> WIPOffset<fb::Kind17375Parsed<'a>> {
    let mints = build_string_vector(builder, v.mints());
    let p2pk_priv_key = v.p2pk_priv_key().map(|s| builder.create_string(s));
    let p2pk_pub_key = v.p2pk_pub_key().map(|s| builder.create_string(s));
    fb::Kind17375Parsed::create(
        builder,
        &fb::Kind17375ParsedArgs {
            mints: Some(mints),
            p2pk_priv_key,
            p2pk_pub_key,
            decrypted: v.decrypted(),
        },
    )
}

fn build_kind7374<'a>(
    builder: &mut FlatBufferBuilder<'a>,
    v: fb::Kind7374Parsed<'_>,
) -> WIPOffset<fb::Kind7374Parsed<'a>> {
    let quote_id = builder.create_string(v.quote_id());
    let mint_url = builder.create_string(v.mint_url());
    fb::Kind7374Parsed::create(
        builder,
        &fb::Kind7374ParsedArgs {
            quote_id: Some(quote_id),
            mint_url: Some(mint_url),
            expiration: v.expiration(),
        },
    )
}

fn build_kind7375<'a>(
    builder: &mut FlatBufferBuilder<'a>,
    v: fb::Kind7375Parsed<'_>,
) -> WIPOffset<fb::Kind7375Parsed<'a>> {
    let mint_url = builder.create_string(v.mint_url());
    let proofs = build_proof_vector(builder, v.proofs());
    let deleted_ids = v.deleted_ids().map(|rv| build_string_vector(builder, rv));
    fb::Kind7375Parsed::create(
        builder,
        &fb::Kind7375ParsedArgs {
            mint_url: Some(mint_url),
            proofs: Some(proofs),
            deleted_ids,
            decrypted: v.decrypted(),
        },
    )
}

fn build_kind7376<'a>(
    builder: &mut FlatBufferBuilder<'a>,
    v: fb::Kind7376Parsed<'_>,
) -> WIPOffset<fb::Kind7376Parsed<'a>> {
    let direction = builder.create_string(v.direction());
    let created_events = v
        .created_events()
        .map(|rv| build_string_vector(builder, rv));
    let destroyed_events = v
        .destroyed_events()
        .map(|rv| build_string_vector(builder, rv));
    let redeemed_events = v
        .redeemed_events()
        .map(|rv| build_string_vector(builder, rv));
    let tags = v.tags().map(|tv| build_history_tag_vector(builder, tv));
    fb::Kind7376Parsed::create(
        builder,
        &fb::Kind7376ParsedArgs {
            direction: Some(direction),
            amount: v.amount(),
            created_events,
            destroyed_events,
            redeemed_events,
            tags,
            decrypted: v.decrypted(),
        },
    )
}

fn build_kind9321<'a>(
    builder: &mut FlatBufferBuilder<'a>,
    v: fb::Kind9321Parsed<'_>,
) -> WIPOffset<fb::Kind9321Parsed<'a>> {
    let id = builder.create_string(v.id());
    let recipient = builder.create_string(v.recipient());
    let sender = builder.create_string(v.sender());
    let event_id = v.event_id().map(|s| builder.create_string(s));
    let mint_url = builder.create_string(v.mint_url());
    let proofs = build_proof_vector(builder, v.proofs());
    let comment = v.comment().map(|s| builder.create_string(s));
    let p2pk_pubkey = v.p2pk_pubkey().map(|s| builder.create_string(s));
    fb::Kind9321Parsed::create(
        builder,
        &fb::Kind9321ParsedArgs {
            id: Some(id),
            amount: v.amount(),
            recipient: Some(recipient),
            sender: Some(sender),
            event_id,
            mint_url: Some(mint_url),
            redeemed: v.redeemed(),
            proofs: Some(proofs),
            comment,
            is_p2pk_locked: v.is_p2pk_locked(),
            p2pk_pubkey,
        },
    )
}

fn build_kind9735<'a>(
    builder: &mut FlatBufferBuilder<'a>,
    v: fb::Kind9735Parsed<'_>,
) -> WIPOffset<fb::Kind9735Parsed<'a>> {
    let id = builder.create_string(v.id());
    let content = builder.create_string(v.content());
    let bolt11 = builder.create_string(v.bolt11());
    let sender = builder.create_string(v.sender());
    let recipient = builder.create_string(v.recipient());
    let event = v.event().map(|s| builder.create_string(s));
    let event_coordinate = v.event_coordinate().map(|s| builder.create_string(s));
    let preimage = v.preimage().map(|s| builder.create_string(s));
    let description = build_zap_request(builder, v.description());
    fb::Kind9735Parsed::create(
        builder,
        &fb::Kind9735ParsedArgs {
            id: Some(id),
            amount: v.amount(),
            content: Some(content),
            bolt11: Some(bolt11),
            preimage,
            sender: Some(sender),
            recipient: Some(recipient),
            event,
            event_coordinate,
            timestamp: v.timestamp(),
            valid: v.valid(),
            description: Some(description),
        },
    )
}

fn build_parsed_union<'a>(
    builder: &mut FlatBufferBuilder<'a>,
    pe: fb::ParsedEvent<'_>,
) -> (
    fb::ParsedData,
    Option<WIPOffset<flatbuffers::UnionWIPOffset>>,
) {
    match pe.parsed_type() {
        fb::ParsedData::Kind0Parsed => {
            let v = pe.parsed_as_kind_0_parsed().unwrap();
            let off = build_kind0(builder, v);
            (fb::ParsedData::Kind0Parsed, Some(off.as_union_value()))
        }
        fb::ParsedData::Kind1Parsed => {
            let v = pe.parsed_as_kind_1_parsed().unwrap();
            let off = build_kind1(builder, v);
            (fb::ParsedData::Kind1Parsed, Some(off.as_union_value()))
        }
        fb::ParsedData::Kind3Parsed => {
            let v = pe.parsed_as_kind_3_parsed().unwrap();
            let off = build_kind3(builder, v);
            (fb::ParsedData::Kind3Parsed, Some(off.as_union_value()))
        }
        fb::ParsedData::Kind4Parsed => {
            let v = pe.parsed_as_kind_4_parsed().unwrap();
            let off = build_kind4(builder, v);
            (fb::ParsedData::Kind4Parsed, Some(off.as_union_value()))
        }
        fb::ParsedData::Kind6Parsed => {
            let v = pe.parsed_as_kind_6_parsed().unwrap();
            let off = build_kind6(builder, v);
            (fb::ParsedData::Kind6Parsed, Some(off.as_union_value()))
        }
        fb::ParsedData::Kind7Parsed => {
            let v = pe.parsed_as_kind_7_parsed().unwrap();
            let off = build_kind7(builder, v);
            (fb::ParsedData::Kind7Parsed, Some(off.as_union_value()))
        }
        fb::ParsedData::Kind17Parsed => {
            let v = pe.parsed_as_kind_17_parsed().unwrap();
            let off = build_kind17(builder, v);
            (fb::ParsedData::Kind17Parsed, Some(off.as_union_value()))
        }
        fb::ParsedData::Kind10002Parsed => {
            let v = pe.parsed_as_kind_10002_parsed().unwrap();
            let off = build_kind10002(builder, v);
            (fb::ParsedData::Kind10002Parsed, Some(off.as_union_value()))
        }
        fb::ParsedData::Kind10019Parsed => {
            let v = pe.parsed_as_kind_10019_parsed().unwrap();
            let off = build_kind10019(builder, v);
            (fb::ParsedData::Kind10019Parsed, Some(off.as_union_value()))
        }
        fb::ParsedData::Kind17375Parsed => {
            let v = pe.parsed_as_kind_17375_parsed().unwrap();
            let off = build_kind17375(builder, v);
            (fb::ParsedData::Kind17375Parsed, Some(off.as_union_value()))
        }
        fb::ParsedData::Kind7374Parsed => {
            let v = pe.parsed_as_kind_7374_parsed().unwrap();
            let off = build_kind7374(builder, v);
            (fb::ParsedData::Kind7374Parsed, Some(off.as_union_value()))
        }
        fb::ParsedData::Kind7375Parsed => {
            let v = pe.parsed_as_kind_7375_parsed().unwrap();
            let off = build_kind7375(builder, v);
            (fb::ParsedData::Kind7375Parsed, Some(off.as_union_value()))
        }
        fb::ParsedData::Kind7376Parsed => {
            let v = pe.parsed_as_kind_7376_parsed().unwrap();
            let off = build_kind7376(builder, v);
            (fb::ParsedData::Kind7376Parsed, Some(off.as_union_value()))
        }
        fb::ParsedData::Kind9321Parsed => {
            let v = pe.parsed_as_kind_9321_parsed().unwrap();
            let off = build_kind9321(builder, v);
            (fb::ParsedData::Kind9321Parsed, Some(off.as_union_value()))
        }
        fb::ParsedData::Kind9735Parsed => {
            let v = pe.parsed_as_kind_9735_parsed().unwrap();
            let off = build_kind9735(builder, v);
            (fb::ParsedData::Kind9735Parsed, Some(off.as_union_value()))
        }
        fb::ParsedData::NONE => (fb::ParsedData::NONE, None),
        _ => (fb::ParsedData::NONE, None),
    }
}

//
// ParsedEvent rebuild (includes parsed union, requests, relays, tags)
//

fn rebuild_parsed_event<'a>(
    builder: &mut FlatBufferBuilder<'a>,
    pe: fb::ParsedEvent<'_>,
) -> WIPOffset<fb::ParsedEvent<'a>> {
    let id = builder.create_string(pe.id());
    let pubkey = builder.create_string(pe.pubkey());
    let tags_vec = build_tags_vector(builder, pe.tags());
    let requests_vec = pe.requests().map(|rv| build_requests_vector(builder, rv));
    let relays_vec = pe.relays().map(|rv| build_string_vector(builder, rv));

    let (parsed_type, parsed_union_opt) = build_parsed_union(builder, pe);

    fb::ParsedEvent::create(
        builder,
        &fb::ParsedEventArgs {
            id: Some(id),
            pubkey: Some(pubkey),
            kind: pe.kind(),
            created_at: pe.created_at(),
            parsed_type,
            parsed: parsed_union_opt,
            requests: requests_vec,
            relays: relays_vec,
            tags: Some(tags_vec),
        },
    )
}

//
// Public API
//

/// Rebuild a WorkerMessage around either a ParsedEvent or a NostrEvent root buffer,
/// injecting the provided `sub_id`. For ParsedEvent, this reconstructs the parsed union,
/// requests, relays, and tags.
pub fn wrap_event_with_worker_message(sub_id: &str, bytes: &[u8]) -> Option<Vec<u8>> {
    // Try ParsedEvent first
    if let Ok(pe) = flatbuffers::root::<fb::ParsedEvent>(bytes) {
        let mut fbb = FlatBufferBuilder::new();
        let sid = fbb.create_string(sub_id);

        // Rebuild full ParsedEvent (including parsed union)
        let parsed_event = rebuild_parsed_event(&mut fbb, pe);

        let msg = fb::WorkerMessage::create(
            &mut fbb,
            &fb::WorkerMessageArgs {
                sub_id: Some(sid),
                url: None,
                type_: fb::MessageType::ParsedNostrEvent,
                content_type: fb::Message::ParsedEvent,
                content: Some(parsed_event.as_union_value()),
            },
        );
        fbb.finish(msg, None);
        return Some(fbb.finished_data().to_vec());
    }

    // Fallback: NostrEvent
    if let Ok(ne) = flatbuffers::root::<fb::NostrEvent>(bytes) {
        let mut fbb = FlatBufferBuilder::new();
        let sid = fbb.create_string(sub_id);

        let id = fbb.create_string(ne.id());
        let pubkey = fbb.create_string(ne.pubkey());
        let content = fbb.create_string(ne.content());
        let sig = fbb.create_string(ne.sig());
        let tags_vec = build_tags_vector(&mut fbb, ne.tags());

        let nostr_event = fb::NostrEvent::create(
            &mut fbb,
            &fb::NostrEventArgs {
                id: Some(id),
                pubkey: Some(pubkey),
                kind: ne.kind(),
                content: Some(content),
                tags: Some(tags_vec),
                created_at: ne.created_at(),
                sig: Some(sig),
            },
        );

        let msg = fb::WorkerMessage::create(
            &mut fbb,
            &fb::WorkerMessageArgs {
                sub_id: Some(sid),
                url: None,
                type_: fb::MessageType::NostrEvent,
                content_type: fb::Message::NostrEvent,
                content: Some(nostr_event.as_union_value()),
            },
        );
        fbb.finish(msg, None);
        return Some(fbb.finished_data().to_vec());
    }

    None
}

/// Same as wrap_event_with_worker_message but uses a caller-provided builder to avoid
/// per-event allocations. Call finished_data() immediately, then builder.reset().
pub fn wrap_event_with_worker_message_in<'a>(
    builder: &'a mut FlatBufferBuilder<'a>,
    sub_id: &str,
    bytes: &[u8],
) -> Option<&'a [u8]> {
    builder.reset();

    // Try ParsedEvent first
    if let Ok(pe) = flatbuffers::root::<fb::ParsedEvent>(bytes) {
        let sid = builder.create_string(sub_id);
        let parsed_event = rebuild_parsed_event(builder, pe);
        let msg = fb::WorkerMessage::create(
            builder,
            &fb::WorkerMessageArgs {
                sub_id: Some(sid),
                url: None,
                type_: fb::MessageType::ParsedNostrEvent,
                content_type: fb::Message::ParsedEvent,
                content: Some(parsed_event.as_union_value()),
            },
        );
        builder.finish(msg, None);
        return Some(builder.finished_data());
    }

    // Fallback: NostrEvent
    if let Ok(ne) = flatbuffers::root::<fb::NostrEvent>(bytes) {
        let sid = builder.create_string(sub_id);

        let id = builder.create_string(ne.id());
        let pubkey = builder.create_string(ne.pubkey());
        let content = builder.create_string(ne.content());
        let sig = builder.create_string(ne.sig());
        let tags_vec = build_tags_vector(builder, ne.tags());

        let nostr_event = fb::NostrEvent::create(
            builder,
            &fb::NostrEventArgs {
                id: Some(id),
                pubkey: Some(pubkey),
                kind: ne.kind(),
                content: Some(content),
                tags: Some(tags_vec),
                created_at: ne.created_at(),
                sig: Some(sig),
            },
        );

        let msg = fb::WorkerMessage::create(
            builder,
            &fb::WorkerMessageArgs {
                sub_id: Some(sid),
                url: None,
                type_: fb::MessageType::ParsedNostrEvent,
                content_type: fb::Message::NostrEvent,
                content: Some(nostr_event.as_union_value()),
            },
        );
        builder.finish(msg, None);
        return Some(builder.finished_data());
    }

    None
}
