#ifndef NIPWORKER_H
#define NIPWORKER_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef void (*nipworker_callback)(void *userdata, const uint8_t *bytes, size_t length);

void *nipworker_init(nipworker_callback callback, void *userdata);
void *nipworker_init_with_storage_path(
	nipworker_callback callback,
	void *userdata,
	const char *storage_path
);
void *nipworker_init_with_config(
	nipworker_callback callback,
	void *userdata,
	const char *storage_path,
	const char *default_relays,
	const char *indexer_relays
);
void *nipworker_init_with_options(
	nipworker_callback callback,
	void *userdata,
	const char *storage_path,
	const char *default_relays,
	const char *indexer_relays,
	bool mesh_enabled
);

void nipworker_wake(void *handle);
void nipworker_handle_message(void *handle, const uint8_t *bytes, size_t length);
bool nipworker_subscribe_message(void *handle, const uint8_t *bytes, size_t length);
bool nipworker_publish_message(void *handle, const uint8_t *bytes, size_t length);
void nipworker_set_private_key(void *handle, const char *private_key);

bool nipworker_register_subscription(void *handle, const char *subscription_id, size_t buffer_size);
bool nipworker_register_publish_buffer(void *handle, const char *publish_id, size_t buffer_size);
bool nipworker_retain_subscription(void *handle, const char *subscription_id);
void nipworker_release_subscription(void *handle, const char *subscription_id);
uint8_t *nipworker_subscription_buffer_ptr(void *handle, const char *subscription_id);
size_t nipworker_subscription_buffer_len(void *handle, const char *subscription_id);
void nipworker_cleanup_subscriptions(void *handle);

void *nipworker_mesh_init(void *handle);
bool nipworker_mesh_peer_connected(void *handle, const char *peer, size_t mtu);
void nipworker_mesh_peer_disconnected(void *handle, const char *peer);
bool nipworker_mesh_set_profile_json(void *handle, const char *profile_json);
bool nipworker_mesh_clear_profile(void *handle);
uint8_t *nipworker_mesh_pop_outbound(void *handle, const char *peer, size_t *out_length);
bool nipworker_mesh_receive_fragment(
	void *handle,
	const char *peer,
	const uint8_t *fragment,
	size_t fragment_length
);
void nipworker_mesh_deinit(void *handle);

/* Frees byte buffers delivered to nipworker_callback or returned by mesh_pop_outbound. */
void nipworker_free_bytes(uint8_t *bytes, size_t length);
void nipworker_deinit(void *handle);

#ifdef __cplusplus
}
#endif

#endif
