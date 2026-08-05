#ifndef SWAP_FFI_H
#define SWAP_FFI_H

#include <stdarg.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>

#ifdef __cplusplus
extern "C" {
#endif

/**
 * Callback invoked on each progress event (called from a worker thread).
 */
typedef void (*ProgressCallback)(const char*, void*);

char *swap_ffi_load_env(const char *path);

char *swap_ffi_run_maker(const char *config_json,
                         const char *hashlock_hex,
                         ProgressCallback cb,
                         void *user_data);

char *swap_ffi_run_taker(const char *config_json,
                         const char *preimage_hex,
                         ProgressCallback cb,
                         void *user_data);

char *swap_ffi_refund_lez(const char *config_json,
                          const char *hashlock_hex);

char *swap_ffi_refund_eth(const char *config_json,
                          const char *swap_id_hex);

char *swap_ffi_fetch_balances(const char *config_json);

char *swap_ffi_run_maker_loop(const char *config_json,
                              ProgressCallback cb,
                              void *user_data);

void swap_ffi_stop_maker_loop(void);

char *swap_ffi_default_lez_htlc_program_id(void);

/* Onboarding: key generation + LEZ account init/funding (see swap-ffi/src/lib.rs). */

char *swap_ffi_generate_eth_key(void);

char *swap_ffi_generate_lez_account(void);

char *swap_ffi_lez_ensure_initialized(const char *sequencer_url,
                                      const char *signing_key_hex);

char *swap_ffi_lez_claim_to_target(const char *sequencer_url,
                                   const char *signing_key_hex,
                                   const char *target_lez,
                                   ProgressCallback cb,
                                   void *user_data);

void swap_ffi_free_string(char *ptr);

#ifdef __cplusplus
}
#endif

#endif  /* SWAP_FFI_H */
