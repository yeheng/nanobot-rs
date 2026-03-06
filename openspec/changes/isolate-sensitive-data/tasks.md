## 1. Core Vault Implementation
- [ ] 1.1 Create `nanobot-core/src/vault/mod.rs` with basic structure
- [ ] 1.2 Implement AES-GCM-256 encryption using `ring` or `aes-gcm` crate
- [ ] 1.3 Implement PBKDF2 key derivation from master password
- [ ] 1.4 Implement secure memory handling (mlock, zero-on-drop)
- [ ] 1.5 Add file-based storage at `~/.nanobot/vault/credentials.enc`

## 2. Config Loader Integration
- [ ] 2.1 Add `ref:` prefix detection in config/loader.rs
- [ ] 2.2 Implement automatic vault resolution during load()
- [ ] 2.3 Ensure Debug output remains redacted for resolved values
- [ ] 2.4 Add backward compatibility for non-ref values

## 3. CLI Commands
- [ ] 3.1 Create `nanobot-cli/src/commands/vault.rs`
- [ ] 3.2 Implement `nanobot vault unlock` - unlock with master password
- [ ] 3.3 Implement `nanobot vault set <key> <value>` - store credential
- [ ] 3.4 Implement `nanobot vault get <key>` - retrieve credential
- [ ] 3.5 Implement `nanobot vault list` - list stored keys (not values)
- [ ] 3.6 Implement `nanobot vault status` - check unlock status

## 4. Auth Command Migration
- [ ] 4.1 Modify `auth copilot` to store token in vault
- [ ] 4.2 Write `ref:copilot-token` to config.yaml
- [ ] 4.3 Handle vault-locked errors with user-friendly messages

## 5. Memory Protection
- [ ] 5.1 Implement mlock() wrapper for sensitive buffers
- [ ] 5.2 Add zero-on-drop for all credential containers
- [ ] 5.3 Prevent credentials from being written to tracing logs
- [ ] 5.4 Ensure history_processor never stores resolved ref: values

## 6. Documentation & Testing
- [ ] 6.1 Add unit tests for vault encryption/decryption
- [ ] 6.2 Add integration tests for config loading with refs
- [ ] 6.3 Update user documentation for vault usage
- [ ] 6.4 Add migration guide for existing users
