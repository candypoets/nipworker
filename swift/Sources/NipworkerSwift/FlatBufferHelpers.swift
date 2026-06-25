import Foundation
import FlatBuffers

// MARK: - Subscribe

func buildSubscribeMessage(
    subId: String,
    requests: [RequestObject],
    options: SubscriptionConfig
) -> Data {
    var builder = FlatBufferBuilder(initialSize: 2048)

    let requestOffsets = requests.map { req -> Offset in
        let idsOffset = req.ids?.isEmpty == false ? builder.createVector(ofStrings: req.ids!) : Offset()
        let authorsOffset = req.authors?.isEmpty == false ? builder.createVector(ofStrings: req.authors!) : Offset()
        let kindsOffset = req.kinds?.isEmpty == false ? builder.createVector(req.kinds!) : Offset()
        let tagsOffset: Offset
        if let tags = req.tags, !tags.isEmpty {
            let tagVecOffsets = tags.map { (key, values) -> Offset in
                let items = builder.createVector(ofStrings: [key] + values)
                return nostr_fb_StringVec.createStringVec(&builder, itemsVectorOffset: items)
            }
            tagsOffset = builder.createVector(ofOffsets: tagVecOffsets)
        } else {
            tagsOffset = Offset()
        }
        let searchOffset = req.search.map { builder.create(string: $0) } ?? Offset()
        let relaysOffset = req.relays.isEmpty ? Offset() : builder.createVector(ofStrings: req.relays)

        return nostr_fb_Request.createRequest(
            &builder,
            idsVectorOffset: idsOffset,
            authorsVectorOffset: authorsOffset,
            kindsVectorOffset: kindsOffset,
            tagsVectorOffset: tagsOffset,
            limit: Int32(req.limit ?? 0),
            since: Int32(req.since ?? 0),
            until: Int32(req.until ?? 0),
            searchOffset: searchOffset,
            relaysVectorOffset: relaysOffset,
            closeOnEose: req.closeOnEOSE ?? false,
            cacheFirst: req.cacheFirst ?? true,
            noCache: req.noCache ?? false,
            maxRelays: req.maxRelays ?? 0,
            cacheOnly: options.cacheOnly
        )
    }

    let pipeOffsets: [Offset] = (options.pipeline?.map { pipeConfig -> Offset in
        let configOffset: Offset
        let configType: nostr_fb_PipeConfig
        switch pipeConfig.kind {
        case .muteFilter:
            configType = .mutefilterpipeconfig
            let start = nostr_fb_MuteFilterPipeConfig.startMuteFilterPipeConfig(&builder)
            configOffset = nostr_fb_MuteFilterPipeConfig.endMuteFilterPipeConfig(&builder, start: start)
        case .parse:
            configType = .parsepipeconfig
            let start = nostr_fb_ParsePipeConfig.startParsePipeConfig(&builder)
            configOffset = nostr_fb_ParsePipeConfig.endParsePipeConfig(&builder, start: start)
        case .saveToDb:
            configType = .savetodbpipeconfig
            let start = nostr_fb_SaveToDbPipeConfig.startSaveToDbPipeConfig(&builder)
            configOffset = nostr_fb_SaveToDbPipeConfig.endSaveToDbPipeConfig(&builder, start: start)
        case .serializeEvents(let sid):
            configType = .serializeeventspipeconfig
            let sidOffset = builder.create(string: sid)
            configOffset = nostr_fb_SerializeEventsPipeConfig.createSerializeEventsPipeConfig(
                &builder,
                subscriptionIdOffset: sidOffset
            )
        case .kindFilter(let kinds):
            configType = .kindfilterpipeconfig
            let kindsOffset = builder.createVector(kinds)
            configOffset = nostr_fb_KindFilterPipeConfig.createKindFilterPipeConfig(
                &builder,
                kindsVectorOffset: kindsOffset
            )
        case .counter(let kinds, let pubkey):
            configType = .counterpipeconfig
            let kindsOffset = builder.createVector(kinds)
            let pubkeyOffset = builder.create(string: pubkey)
            configOffset = nostr_fb_CounterPipeConfig.createCounterPipeConfig(
                &builder,
                kindsVectorOffset: kindsOffset,
                pubkeyOffset: pubkeyOffset
            )
        case .npubLimiter(let kind, let limitPerNpub, let maxTotalNpubs):
            configType = .npublimiterpipeconfig
            configOffset = nostr_fb_NpubLimiterPipeConfig.createNpubLimiterPipeConfig(
                &builder,
                kind: kind,
                limitPerNpub: limitPerNpub,
                maxTotalNpubs: maxTotalNpubs
            )
        case .proofVerification(let maxProofs):
            configType = .proofverificationpipeconfig
            configOffset = nostr_fb_ProofVerificationPipeConfig.createProofVerificationPipeConfig(
                &builder,
                maxProofs: maxProofs
            )
        }
        return nostr_fb_Pipe.createPipe(&builder, configType: configType, configOffset: configOffset)
    }) ?? [
        defaultMuteFilterPipe(&builder),
        defaultParsePipe(&builder),
        defaultSaveToDbPipe(&builder),
        defaultSerializeEventsPipe(&builder, subId: subId)
    ]

    let pipelineOffset = nostr_fb_PipelineConfig.createPipelineConfig(
        &builder,
        pipesVectorOffset: builder.createVector(ofOffsets: pipeOffsets)
    )

    let configOffset = nostr_fb_SubscriptionConfig.createSubscriptionConfig(
        &builder,
        pipelineOffset: pipelineOffset,
        closeOnEose: options.closeOnEose,
        cacheFirst: options.cacheFirst,
        timeoutMs: options.timeoutMs ?? 0,
        maxEvents: options.maxEvents ?? 0,
        skipCache: options.skipCache,
        force: options.force,
        bytesPerEvent: options.bytesPerEvent,
        isSlow: options.isSlow,
        paginationOffset: options.pagination.map { builder.create(string: $0) } ?? Offset(),
        cacheOnly: options.cacheOnly
    )

    let subIdOffset = builder.create(string: subId)
    let subscribeOffset = nostr_fb_Subscribe.createSubscribe(
        &builder,
        subscriptionIdOffset: subIdOffset,
        requestsVectorOffset: builder.createVector(ofOffsets: requestOffsets),
        configOffset: configOffset
    )

    let mainOffset = nostr_fb_MainMessage.createMainMessage(
        &builder,
        contentType: .subscribe,
        contentOffset: subscribeOffset
    )

    builder.finish(offset: mainOffset)
    return Data(builder.data)
}

private func defaultMuteFilterPipe(_ builder: inout FlatBufferBuilder) -> Offset {
    let start = nostr_fb_MuteFilterPipeConfig.startMuteFilterPipeConfig(&builder)
    let configOffset = nostr_fb_MuteFilterPipeConfig.endMuteFilterPipeConfig(&builder, start: start)
    return nostr_fb_Pipe.createPipe(&builder, configType: .mutefilterpipeconfig, configOffset: configOffset)
}

private func defaultParsePipe(_ builder: inout FlatBufferBuilder) -> Offset {
    let start = nostr_fb_ParsePipeConfig.startParsePipeConfig(&builder)
    let configOffset = nostr_fb_ParsePipeConfig.endParsePipeConfig(&builder, start: start)
    return nostr_fb_Pipe.createPipe(&builder, configType: .parsepipeconfig, configOffset: configOffset)
}

private func defaultSaveToDbPipe(_ builder: inout FlatBufferBuilder) -> Offset {
    let start = nostr_fb_SaveToDbPipeConfig.startSaveToDbPipeConfig(&builder)
    let configOffset = nostr_fb_SaveToDbPipeConfig.endSaveToDbPipeConfig(&builder, start: start)
    return nostr_fb_Pipe.createPipe(&builder, configType: .savetodbpipeconfig, configOffset: configOffset)
}

private func defaultSerializeEventsPipe(_ builder: inout FlatBufferBuilder, subId: String) -> Offset {
    let sidOffset = builder.create(string: subId)
    let configOffset = nostr_fb_SerializeEventsPipeConfig.createSerializeEventsPipeConfig(
        &builder,
        subscriptionIdOffset: sidOffset
    )
    return nostr_fb_Pipe.createPipe(&builder, configType: .serializeeventspipeconfig, configOffset: configOffset)
}

// MARK: - Unsubscribe

func buildUnsubscribeMessage(subId: String) -> Data {
    var builder = FlatBufferBuilder(initialSize: 256)
    let subIdOffset = builder.create(string: subId)
    let unsubOffset = nostr_fb_Unsubscribe.createUnsubscribe(&builder, subscriptionIdOffset: subIdOffset)
    let mainOffset = nostr_fb_MainMessage.createMainMessage(&builder, contentType: .unsubscribe, contentOffset: unsubOffset)
    builder.finish(offset: mainOffset)
    return Data(builder.data)
}

// MARK: - Publish

func buildPublishMessage(
    publishId: String,
    event: NostrEvent,
    defaultRelays: [String],
    optimisticSubIds: [String]
) -> Data {
    var builder = FlatBufferBuilder(initialSize: 2048)

    let contentOffset = builder.create(string: event.content)
    let tagOffsets = event.tags.map { tag -> Offset in
        let items = builder.createVector(ofStrings: tag)
        return nostr_fb_StringVec.createStringVec(&builder, itemsVectorOffset: items)
    }
    let tagsOffset = builder.createVector(ofOffsets: tagOffsets)
    let templateOffset = nostr_fb_Template.createTemplate(
        &builder,
        kind: event.kind,
        createdAt: Int32(event.createdAt),
        contentOffset: contentOffset,
        tagsVectorOffset: tagsOffset
    )

    let publishIdOffset = builder.create(string: publishId)
    let relaysOffset = defaultRelays.isEmpty ? Offset() : builder.createVector(ofStrings: defaultRelays)
    let optimisticSubIdsOffset = optimisticSubIds.isEmpty ? Offset() : builder.createVector(ofStrings: optimisticSubIds)

    let publishOffset = nostr_fb_Publish.createPublish(
        &builder,
        publishIdOffset: publishIdOffset,
        templateOffset: templateOffset,
        relaysVectorOffset: relaysOffset,
        optimisticSubidsVectorOffset: optimisticSubIdsOffset
    )

    let mainOffset = nostr_fb_MainMessage.createMainMessage(&builder, contentType: .publish, contentOffset: publishOffset)
    builder.finish(offset: mainOffset)
    return Data(builder.data)
}

// MARK: - SignEvent

func buildSignEventMessage(template: EventTemplate) -> Data {
    var builder = FlatBufferBuilder(initialSize: 2048)

    let contentOffset = builder.create(string: template.content)
    let tagOffsets = template.tags.map { tag -> Offset in
        let items = builder.createVector(ofStrings: tag)
        return nostr_fb_StringVec.createStringVec(&builder, itemsVectorOffset: items)
    }
    let tagsOffset = builder.createVector(ofOffsets: tagOffsets)
    let templateOffset = nostr_fb_Template.createTemplate(
        &builder,
        kind: template.kind,
        createdAt: Int32(Date().timeIntervalSince1970),
        contentOffset: contentOffset,
        tagsVectorOffset: tagsOffset
    )

    let signOffset = nostr_fb_SignEvent.createSignEvent(&builder, templateOffset: templateOffset)
    let mainOffset = nostr_fb_MainMessage.createMainMessage(&builder, contentType: .signevent, contentOffset: signOffset)
    builder.finish(offset: mainOffset)
    return Data(builder.data)
}

// MARK: - GetPublicKey

func buildGetPublicKeyMessage() -> Data {
    var builder = FlatBufferBuilder(initialSize: 256)
    let getPubkeyOffset = nostr_fb_GetPublicKey.endGetPublicKey(&builder, start: nostr_fb_GetPublicKey.startGetPublicKey(&builder))
    let mainOffset = nostr_fb_MainMessage.createMainMessage(&builder, contentType: .getpublickey, contentOffset: getPubkeyOffset)
    builder.finish(offset: mainOffset)
    return Data(builder.data)
}

// MARK: - PrivateKey signer

func buildSetPrivateKeyMessage(secret: String) -> Data {
    var builder = FlatBufferBuilder(initialSize: 256)
    let secretOffset = builder.create(string: secret)
    let pkOffset = nostr_fb_PrivateKey.createPrivateKey(&builder, privateKeyOffset: secretOffset)
    let signerOffset = nostr_fb_SetSigner.createSetSigner(&builder, signerTypeType: .privatekey, signerTypeOffset: pkOffset)
    let mainOffset = nostr_fb_MainMessage.createMainMessage(&builder, contentType: .setsigner, contentOffset: signerOffset)
    builder.finish(offset: mainOffset)
    return Data(builder.data)
}
