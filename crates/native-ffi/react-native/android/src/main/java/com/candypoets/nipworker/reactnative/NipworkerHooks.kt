package com.candypoets.nipworker.reactnative

import com.google.flatbuffers.FlatBufferBuilder
import java.io.Closeable
import java.nio.ByteBuffer
import java.nio.ByteOrder
import nostr.fb.MainContent
import nostr.fb.MainMessage
import nostr.fb.MessageType
import nostr.fb.MuteFilterPipeConfig
import nostr.fb.ParsePipeConfig
import nostr.fb.Pipe
import nostr.fb.PipeConfig
import nostr.fb.PipelineConfig
import nostr.fb.Publish
import nostr.fb.Request
import nostr.fb.SaveToDbPipeConfig
import nostr.fb.SerializeEventsPipeConfig
import nostr.fb.StringVec
import nostr.fb.Subscribe
import nostr.fb.SubscriptionConfig
import nostr.fb.Template
import nostr.fb.WorkerMessage

data class NipworkerRequest(
	val ids: List<String> = emptyList(),
	val authors: List<String> = emptyList(),
	val kinds: List<Int> = emptyList(),
	val tags: Map<String, List<String>> = emptyMap(),
	val since: Int = 0,
	val until: Int = 0,
	val limit: Int = 0,
	val search: String? = null,
	val relays: List<String> = emptyList(),
	val closeOnEose: Boolean = false,
	val cacheFirst: Boolean = true,
	val noCache: Boolean = false,
	val maxRelays: Int = 0,
)

data class NipworkerSubscriptionOptions(
	val closeOnEose: Boolean = false,
	val cacheFirst: Boolean = true,
	val timeoutMs: Long = 0,
	val maxEvents: Long = 0,
	val skipCache: Boolean = false,
	val force: Boolean = false,
	val bytesPerEvent: Long = 3072,
	val isSlow: Boolean = false,
	val pagination: String? = null,
	val cacheOnly: Boolean = false,
)

data class NipworkerEventTemplate(
	val kind: Int,
	val content: String,
	val tags: List<List<String>> = emptyList(),
	val createdAt: Int = 0,
)

class NipworkerWorkerMessage(private val bytes: ByteArray) {
	private val buffer: ByteBuffer =
		ByteBuffer.wrap(bytes).order(ByteOrder.LITTLE_ENDIAN)

	val message: WorkerMessage = WorkerMessage.getRootAsWorkerMessage(buffer)
	val subId: String? get() = message.subId()
	val url: String? get() = message.url()
	val type: Long get() = message.type()
	val contentType: Byte get() = message.contentType()

	fun bytes(): ByteArray = bytes
}

class NipworkerHookHandle internal constructor(private val cancelAction: () -> Unit) : Closeable {
	@Volatile
	private var closed = false

	override fun close() {
		if (!closed) {
			closed = true
			cancelAction()
		}
	}

	fun cancel() = close()
}

fun NipworkerRuntime.useSubscription(
	subscriptionId: String,
	requests: List<NipworkerRequest>,
	options: NipworkerSubscriptionOptions = NipworkerSubscriptionOptions(),
	onMessages: (List<NipworkerWorkerMessage>) -> Unit,
): NipworkerHookHandle {
	val buffer = retainSubscriptionBuffer(subscriptionId)
		?: subscribe(buildSubscribeMessage(subscriptionId, requests, options), subscriptionId)
	var lastReadPosition = 4

	fun drain() {
		val messages = buffer?.readWorkerMessages(lastReadPosition).orEmpty()
		if (messages.isNotEmpty()) {
			lastReadPosition += messages.sumOf { it.bytes().size + 4 }
			onMessages(messages)
		}
	}

	val listener = addListener(subscriptionId) {
		drain()
	}

	drain()

	return NipworkerHookHandle {
		listener()
		releaseSubscription(subscriptionId)
	}
}

fun NipworkerRuntime.usePublish(
	publishId: String,
	event: NipworkerEventTemplate,
	defaultRelays: List<String> = emptyList(),
	optimisticSubIds: List<String> = emptyList(),
	onStatus: (NipworkerWorkerMessage) -> Unit,
): NipworkerHookHandle {
	val buffer = publish(buildPublishMessage(publishId, event, defaultRelays, optimisticSubIds), publishId)
	var lastReadPosition = 4

	fun drain() {
		val messages = buffer?.readWorkerMessages(lastReadPosition).orEmpty()
		if (messages.isNotEmpty()) {
			lastReadPosition += messages.sumOf { it.bytes().size + 4 }
			messages
				.filter { it.type != MessageType.ParsedNostrEvent }
				.forEach(onStatus)
		}
	}

	val listener = addListener(publishId) {
		drain()
	}

	drain()

	return NipworkerHookHandle {
		listener()
		releaseSubscription(publishId)
	}
}

private fun ByteBuffer.readWorkerMessages(fromPosition: Int): List<NipworkerWorkerMessage> {
	val source = duplicate().order(ByteOrder.LITTLE_ENDIAN)
	if (source.capacity() < 4) return emptyList()
	val writePosition = source.getInt(0).coerceAtMost(source.capacity())
	var position = fromPosition
	val messages = mutableListOf<NipworkerWorkerMessage>()
	while (position + 4 <= writePosition) {
		val messageLength = source.getInt(position)
		if (messageLength <= 0 || position + 4 + messageLength > writePosition) break
		val bytes = ByteArray(messageLength)
		source.position(position + 4)
		source.get(bytes)
		messages.add(NipworkerWorkerMessage(bytes))
		position += 4 + messageLength
	}
	return messages
}

private fun buildSubscribeMessage(
	subId: String,
	requests: List<NipworkerRequest>,
	options: NipworkerSubscriptionOptions,
): ByteArray {
	val builder = FlatBufferBuilder(2048)
	val requestOffsets = requests.map { request -> buildRequest(builder, request, options) }.toIntArray()
	val requestsOffset = Subscribe.createRequestsVector(builder, requestOffsets)
	val pipelineOffset = buildDefaultPipeline(builder, subId)
	val paginationOffset = options.pagination?.let { builder.createString(it) } ?: 0
	val configOffset = SubscriptionConfig.createSubscriptionConfig(
		builder,
		pipelineOffset,
		options.closeOnEose,
		options.cacheFirst,
		options.timeoutMs,
		options.maxEvents,
		options.skipCache,
		options.force,
		options.bytesPerEvent,
		options.isSlow,
		paginationOffset,
		options.cacheOnly,
	)
	val subscribeOffset = Subscribe.createSubscribe(
		builder,
		builder.createString(subId),
		requestsOffset,
		configOffset,
	)
	val root = MainMessage.createMainMessage(builder, MainContent.Subscribe, subscribeOffset)
	builder.finish(root)
	return builder.sizedByteArray()
}

private fun buildRequest(
	builder: FlatBufferBuilder,
	request: NipworkerRequest,
	options: NipworkerSubscriptionOptions,
): Int {
	val idsOffset = stringVector(builder, request.ids)
	val authorsOffset = stringVector(builder, request.authors)
	val kindsOffset = if (request.kinds.isEmpty()) 0 else Request.createKindsVector(builder, request.kinds.toIntArray())
	val tagOffsets = request.tags.map { (key, values) ->
		StringVec.createStringVec(builder, stringVector(builder, listOf(key) + values))
	}.toIntArray()
	val tagsOffset = if (tagOffsets.isEmpty()) 0 else Request.createTagsVector(builder, tagOffsets)
	val searchOffset = request.search?.let { builder.createString(it) } ?: 0
	val relaysOffset = stringVector(builder, request.relays)
	return Request.createRequest(
		builder,
		idsOffset,
		authorsOffset,
		kindsOffset,
		tagsOffset,
		request.limit,
		request.since,
		request.until,
		searchOffset,
		relaysOffset,
		request.closeOnEose,
		request.cacheFirst,
		request.noCache,
		request.maxRelays,
		options.cacheOnly,
	)
}

private fun buildDefaultPipeline(builder: FlatBufferBuilder, subId: String): Int {
	val muteStart = MuteFilterPipeConfig.startMuteFilterPipeConfig(builder)
	val muteConfig = MuteFilterPipeConfig.endMuteFilterPipeConfig(builder)
	val mutePipe = Pipe.createPipe(builder, PipeConfig.MuteFilterPipeConfig, muteConfig)

	val parseStart = ParsePipeConfig.startParsePipeConfig(builder)
	val parseConfig = ParsePipeConfig.endParsePipeConfig(builder)
	val parsePipe = Pipe.createPipe(builder, PipeConfig.ParsePipeConfig, parseConfig)

	val saveStart = SaveToDbPipeConfig.startSaveToDbPipeConfig(builder)
	val saveConfig = SaveToDbPipeConfig.endSaveToDbPipeConfig(builder)
	val savePipe = Pipe.createPipe(builder, PipeConfig.SaveToDbPipeConfig, saveConfig)

	val serializeConfig = SerializeEventsPipeConfig.createSerializeEventsPipeConfig(builder, builder.createString(subId))
	val serializePipe = Pipe.createPipe(builder, PipeConfig.SerializeEventsPipeConfig, serializeConfig)

	return PipelineConfig.createPipelineConfig(
		builder,
		PipelineConfig.createPipesVector(builder, intArrayOf(mutePipe, parsePipe, savePipe, serializePipe)),
	)
}

private fun buildPublishMessage(
	publishId: String,
	event: NipworkerEventTemplate,
	defaultRelays: List<String>,
	optimisticSubIds: List<String>,
): ByteArray {
	val builder = FlatBufferBuilder(1024)
	val tags = event.tags.map { tag ->
		StringVec.createStringVec(builder, stringVector(builder, tag))
	}.toIntArray()
	val tagsOffset = Template.createTagsVector(builder, tags)
	val templateOffset = Template.createTemplate(
		builder,
		event.kind,
		event.createdAt,
		builder.createString(event.content),
		tagsOffset,
	)
	val publishOffset = Publish.createPublish(
		builder,
		builder.createString(publishId),
		templateOffset,
		stringVector(builder, defaultRelays),
		stringVector(builder, optimisticSubIds),
	)
	val root = MainMessage.createMainMessage(builder, MainContent.Publish, publishOffset)
	builder.finish(root)
	return builder.sizedByteArray()
}

private fun stringVector(builder: FlatBufferBuilder, values: List<String>): Int {
	if (values.isEmpty()) return 0
	val offsets = values.map { builder.createString(it) }.toIntArray()
	return builder.createVectorOfTables(offsets)
}
