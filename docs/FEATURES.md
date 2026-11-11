# Features (the scope)

Last verified: 2026-11-11 @ d3bf51b

**This file is the scope. Anything not listed in an in-scope table is out unless the maintainer adds it** (decision of 2026-10-28). Every feature must work on Windows, macOS and Linux before it is marked Done, except where a row says otherwise.

- **Status values:** `Planned`, `In progress`, `Done`.
- **Rules:**
  - A `Done` row must list the tests that prove it. `cargo xtask docs-check` fails if a listed test doesn't exist.
  - IDs never change or get reused.

## App shell (F-APP)

| ID | Feature | Status | Crate | Tests |
|---|---|---|---|---|
| F-APP-001 | Top tab strip: Vault, SFTP, session tabs, "+". Drag to reorder, middle-click to close, rename, context menu (Reconnect, Duplicate, Close) | In progress | app | `the_fixed_tabs_cannot_be_closed_and_closing_moves_focus_left`, `the_tabs_x_stops_its_live_session_before_removing_the_tab`, `a_tab_moves_among_the_sessions_and_the_one_in_front_stays_in_front`, `a_tab_dragged_past_its_neighbour_takes_its_place`, `the_sftp_tab_picks_a_host_and_goes_back_to_the_picker`, `a_tabs_menu_closes_the_tabs_to_its_right_or_all_the_others`, `many_tabs_share_the_strip_and_none_shrinks_to_nothing`, `squeezed_tabs_keep_a_readable_part_of_their_names`, `tabs_past_the_strips_end_wait_in_the_all_tabs_menu_and_come_back_when_picked` |
| F-APP-002 | Status dots on tabs and panes: unread output, input needed, error | In progress | app | `the_dot_shows_the_most_urgent_state`, `pane_dots_take_the_themes_accent_warning_and_error_colours` |
| F-APP-003 | Vault left nav: Hosts, Keychain, Port Forwarding, Snippets, Known Hosts, Logs, with Settings at the bottom | Planned | app | — |
| F-APP-004 | New Tab screen: pinned hosts, recent connections, local shell profiles, saved workspaces; a welcome while the vault is empty | In progress | app, config | `pinned_hosts_head_the_page_in_pin_order_and_are_listed_nowhere_else`, `a_host_pinned_from_a_new_tab_heads_the_next_one_and_a_second_click_unpins_it`, `an_empty_vaults_new_tab_welcomes_and_offers_the_ssh_config_import`, `recent_connections_come_first_then_the_newest_saved_hosts_fill_the_room`, `a_search_lists_every_match_and_at_most_six_recent`, `times_read_as_a_person_would_say_them`, `a_connection_goes_to_the_front_of_the_recent_ones_once_and_old_ones_drop_off`, `the_plus_button_opens_a_new_tab_whose_search_connects_in_its_place` |
| F-APP-005 | Command palette: hosts first, then open tabs, saved items, all app pages and commands. Fuzzy, keyboard-only; named Edit, SFTP, Connect, Open and Go actions | In progress | app | `letters_in_order_match_and_better_matches_come_first`, `a_named_action_scopes_the_search_and_accepts_parentheses`, `search_finds_one_host_in_a_vault_of_five_thousand`, `a_named_palette_action_opens_the_saved_host_editor_from_the_keyboard`, `choosing_a_palette_result_repaints_without_the_blurred_backdrop`, `cancelling_the_palette_returns_keyboard_focus`, `the_palette_lists_every_page_and_opens_host_and_key_editors`, `the_palette_lists_tabs_and_commands_and_runs_them`, `retina_cpu_motion_draws_at_sixty_frames_per_second`, `retina_palette_shadows_draw_within_one_frame` |
| F-APP-006 | Quick connect `user@host:port` from search or the palette, with an option to save it as a host | In progress | app, ssh | `a_typed_target_can_be_saved_as_a_host`, `a_typed_target_is_offered_as_a_quick_connect`, `only_a_target_no_host_is_labelled_with_becomes_a_draft_in_the_group_shown`, `quick_connect_to_a_saved_label_uses_its_details` |
| F-APP-007 | Error dialogs with cause and next step, toasts, banners, "copy diagnostic info" | In progress | app | `failures_come_with_a_next_step_and_copyable_details`, `an_ssh_tab_says_it_is_connecting_until_it_draws`, `the_connecting_card_follows_the_steps_and_waits_on_a_question`, `the_connecting_card_sits_near_the_middle_and_its_log_opens_downward` |
| F-APP-008 | Restore last open tabs on launch (setting, off by default) | In progress | app, config | `tabs_are_listed_for_the_next_launch_and_host_tabs_wait_for_the_vault` |

## Hosts and groups (F-HOST)

| ID | Feature | Status | Crate | Tests |
|---|---|---|---|---|
| F-HOST-001 | Host editor in a right side panel with Save and Cancel. Basic fields: label, address, port (22), username, credential, group, tags. A host left without a username (by itself and its groups) keeps the one it logs in as, so synced devices agree | Planned | app, vault | `a_host_without_a_user_keeps_this_devices_but_never_hides_its_groups`, `saving_an_editor_keeps_what_changed_meanwhile_and_applies_what_was_edited`, `a_host_deleted_while_it_was_edited_is_not_brought_back`, `a_port_is_empty_or_from_1_to_65535`, `the_plus_button_opens_the_saved_hosts_ready_to_search`, `the_editor_panel_stays_inside_the_rounded_content_card` |
| F-HOST-002 | Advanced fields ("Show more"): jump hosts, proxy, startup snippet, env vars, agent forwarding, keepalive, auto-reconnect, terminal theme/font override, legacy algorithms, PQ KEX toggle, session logging, auth order | Planned | app, vault | — |
| F-HOST-003 | Nested groups. Each group sets defaults that hosts inherit per field (unset ≠ empty), and the editor shows where each value comes from | In progress | vault, sync, app | `unset_fields_inherit_from_the_nearest_group_that_sets_them`, `a_hosts_own_value_wins_even_when_it_is_the_default_port`, `deleting_a_group_moves_its_hosts_and_subgroups_up_and_loops_are_refused`, `groups_and_what_hosts_inherit_reach_another_device`, `a_host_in_a_group_connects_with_what_it_inherits`, `move_to_group_puts_the_host_in_the_chosen_group_or_back_on_top`, `deleting_a_host_leaves_nothing_pointing_at_its_id`, `connect_all_opens_every_host_directly_in_the_group`, `deleting_a_group_moves_its_hosts_up_or_takes_them_along` |
| F-HOST-004 | Tags with suggestions, AND filter across tags, global tag delete | In progress | vault, app | `tags_are_counted_and_a_filter_needs_every_chosen_tag`, `a_tag_comes_off_every_host_in_one_go`, `the_tag_filter_works_by_keyboard` |
| F-HOST-005 | Fuzzy search over label, address, user and tags | In progress | app | `search_needs_every_word_somewhere` |
| F-HOST-006 | Grid and list views. Drag hosts into a saved order. Sort by name, last used or address | In progress | app, ui, vault | `a_hovered_card_on_the_lists_top_edge_keeps_the_top_of_its_outline`, `a_focused_card_on_the_lists_edges_shows_its_whole_focus_ring`, `a_list_drop_chooses_before_or_after_the_host_under_the_pointer`, `dragging_a_host_reorders_the_page_and_saves_its_place`, `a_dragged_host_keeps_its_new_place_after_unlock` |
| F-HOST-007 | Multi-select, drag to a group, bulk move/delete, duplicate (from a host's menu or its editor) | In progress | app | `duplicate_saves_a_copy_with_a_new_id_and_the_same_address`, `the_host_editor_duplicates_what_it_shows_and_opens_the_copy`, `move_to_group_puts_the_host_in_the_chosen_group_or_back_on_top` |
| F-HOST-008 | Group "Connect all" opens one tab per host | In progress | app | `connect_all_opens_every_host_directly_in_the_group` |
| F-HOST-009 | OS icon guessed from the SSH server banner (no exec probes), or picked in the host editor; synced with the host | In progress | ssh, vault, app | `the_os_comes_from_the_servers_identification_line_when_it_names_one`, `every_os_has_a_unique_key_a_mark_and_a_tile_its_mark_shows_on`, `what_the_server_says_replaces_a_guess_but_never_a_choice` |
| F-HOST-010 | Copy an `ssh` command for the host | In progress | app | `the_ssh_command_has_port_jump_and_user_but_never_a_password`, `a_copied_ssh_command_quotes_what_a_shell_would_run`, `a_hosts_context_menu_copies_its_ssh_command_and_duplicates_it` |
| F-HOST-011 | Notes on a host: what to know about the server, 2,000 characters at most, in the host editor and synced with the host; agents read them with the hosts and may rewrite them (F-AUTO), and the maintainer gets a toast when one does | In progress | vault, app, control, cli | `notes_are_trimmed_and_cleaned_and_refused_past_their_length`, `notes_save_cleaned_and_an_agents_notes_written_meanwhile_stay_unless_edited_here`, `an_agent_reads_a_hosts_notes_and_rewrites_them_within_their_length`, `a_hosts_notes_are_printed_or_set_and_set_alone_clears_them`, `a_hosts_notes_reach_the_server_sealed_and_arrive_whole`, `a_wrapping_text_area_breaks_lines_at_its_width_and_draws_every_character`, `end_up_and_down_keep_to_wrapped_rows` |
| F-HOST-012 | Connect with one click: a setting, off by default, under which a click on a host in the Vault connects to it, as a row on the New Tab page does; off, a click selects and a double-click connects | In progress | config, app | `one_click_connecting_never_opens_a_host_twice_for_a_double_click`, `a_click_on_a_host_selects_it_or_with_one_click_connecting_connects`, `settings_round_trip_and_bad_files_fall_back_to_defaults` |

## Keychain: every common auth method and key type (F-KEY)

| ID | Feature | Status | Crate | Tests |
|---|---|---|---|---|
| F-KEY-001 | Generate Ed25519 (default), ECDSA P-256/384/521 and RSA 3072/4096 keys, with an optional passphrase and comment | In progress | vault, ssh | `a_generated_key_imports_back_to_the_same_public_key`, `ecdsa_and_rsa_keys_generate_and_decode_for_signing`, `a_keychain_key_logs_in_without_asking_for_a_password` |
| F-KEY-002 | Import private keys in OpenSSH, PEM (PKCS#1/#8, SEC1) and PuTTY PPK v2/v3 formats, by paste, file or drag-drop. Encrypted keys prompt for the passphrase, with an option to save it | In progress | vault, ssh | `an_encrypted_key_needs_its_passphrase_and_is_stored_decrypted`, `keys_are_stored_encrypted_and_deleting_one_detaches_its_hosts`, `a_key_pasted_into_the_keychain_is_saved_under_its_name`, `a_secret_text_area_takes_a_pasted_key_and_draws_only_dots` |
| F-KEY-003 | Copy or save a public key as an `authorized_keys` line. Export the private key re-encrypted, with a warning | In progress | vault, app | `a_generated_key_is_stored_with_its_public_half_and_a_fresh_id` |
| F-KEY-004 | Export a key to a host: append to remote `authorized_keys` (deduplicated, correct permissions) and link the key to the host. Deleting a key hosts log in with takes it off their `authorized_keys` first (on unless unticked); a server that cannot keeps the key in the vault. Works on Windows OpenSSH servers too (cmd or PowerShell; an administrator's key goes to `administrators_authorized_keys`) | In progress | ssh, app | `the_install_commands_quote_the_key_line`, `exec_returns_the_output_and_the_exit_status`, `an_install_succeeds_only_when_the_command_says_so_with_status_zero`, `an_installed_key_becomes_the_hosts_key`, `a_refused_connection_comes_back_as_an_error`, `the_remove_commands_match_the_key_data_and_refuse_anything_else`, `the_key_commands_add_a_key_once_and_take_off_only_it`, `a_removal_counts_only_on_its_own_word_and_a_server_that_kept_it_keeps_the_key`, `a_key_a_host_takes_from_its_group_counts_as_one_it_logs_in_with`, `a_command_for_the_servers_shell_runs_after_asking_which_shell_it_is`, `the_probe_names_the_shell_it_runs_in`, `the_probe_tells_windows_shells_from_unix_ones`, `on_windows_an_administrators_key_goes_where_sshd_reads_it`, `a_windows_command_for_a_long_key_fits_cmds_command_line`, `e2e_the_new_keychain_installs_a_key_and_deleting_it_takes_it_off_the_host` |
| F-KEY-005 | Identities: label + username + password and/or key, linked to hosts and groups. The host's own fields override the identity's | Planned | vault | — |
| F-KEY-006 | OpenSSH user certificates attached to a key. Show principals and validity, and warn before expiry | Planned | vault, ssh | — |
| F-KEY-007 | FIDO2 security keys: generate `sk-ed25519` / `sk-ecdsa` keys (resident and non-resident), import resident keys, require touch and/or PIN | Planned | platform, ssh | — |
| F-KEY-008 | Hardware-bound, non-exportable keys: Windows Hello/TPM and macOS Secure Enclave (P-256). Device-only, never synced. **Not available on Linux** (use FIDO2) | Planned | platform, ssh | — |
| F-KEY-009 | Built-in agent holding vault keys, used for agent forwarding | Planned | ssh | — |
| F-KEY-010 | Use the OS agent (Windows OpenSSH pipe, Pageant, `SSH_AUTH_SOCK`) as an auth source | Planned | ssh, platform | — |
| F-KEY-011 | Edit a saved key: rename it, show its private key on asking (an eye toggles dots and text), copy it as a secret, or paste another key over it to replace it under the same id | In progress | ui, app | `a_saved_key_opens_to_edit_shows_and_copies_its_private_key_on_asking_and_can_be_replaced`, `a_revealed_secret_shows_its_text_yet_never_copies_out_or_reaches_a_screen_reader` |

## Known hosts (F-KNOWN)

| ID | Feature | Status | Crate | Tests |
|---|---|---|---|---|
| F-KNOWN-001 | Known-host entries (host:port, key type, key) stored in the vault and synced | Planned | ssh, vault | — |
| F-KNOWN-002 | Unknown-key prompt: SHA256 fingerprint and key type. Trust and save, connect once, or cancel | In progress | ssh, app | `first_connection_asks_to_trust_the_key_then_the_shell_echoes`, `a_rejected_new_key_stops_before_any_password_is_sent`, `a_key_trusted_once_is_not_remembered`, `a_host_key_that_cannot_be_saved_is_reported_not_forgotten` |
| F-KNOWN-003 | Changed key is a hard block. Show old and new fingerprints; continuing requires explicitly removing the old key | In progress | ssh, app | `a_changed_host_key_is_a_hard_stop`, `a_known_host_presenting_another_kind_of_key_is_a_changed_key`, `a_host_known_by_an_ecdsa_key_is_asked_for_that_key` |
| F-KNOWN-004 | Known Hosts screen: search, delete, multi-select, convert to a host | In progress | ssh, app | `entries_list_hosts_types_and_fingerprints`, `forgetting_removes_only_that_line`, `known_hosts_shows_a_key_trusted_since_it_was_opened` |
| F-KNOWN-005 | Import `~/.ssh/known_hosts`, including hashed entries | In progress | ssh, app | `importing_adds_new_hosts_and_never_a_second_key_for_a_trusted_one`, `a_hashed_line_for_a_host_with_a_key_is_left_out_and_the_check_still_passes` |
| F-KNOWN-006 | Per-host "auto-accept new (never changed) keys" option | Planned | ssh | — |

## SSH connections (F-SSH)

| ID | Feature | Status | Crate | Tests |
|---|---|---|---|---|
| F-SSH-001 | Password auth, including empty passwords | In progress | ssh | `three_wrong_passwords_fail_with_what_was_tried`, `password_login_and_a_shell` |
| F-SSH-002 | Public-key auth with vault keys, prompting for the passphrase when needed. When the server refuses the keys, the password question says so (often the wrong username) | In progress | ssh, session, app | `a_keychain_key_logs_in_without_asking_for_a_password`, `a_refused_key_is_reported_before_the_password_is_asked_for`, `the_password_question_says_whether_the_keys_were_refused` |
| F-SSH-003 | Keyboard-interactive / 2FA dialog: multiple prompts, echo flags, banner | Planned | ssh, app | — |
| F-SSH-004 | Certificate auth | Planned | ssh | — |
| F-SSH-005 | Auth method order per host, plus "choose method at connect" | Planned | ssh, app | — |
| F-SSH-006 | Modern algorithms by default, including `mlkem768x25519-sha256`. Legacy algorithms opt-in per host. PQ KEX toggle | Planned | ssh | `sha1_rsa_signatures_are_never_offered` |
| F-SSH-007 | PTY request with TERM choice (xterm-256color default, xterm, vt100, linux), resize propagation | In progress | ssh | `unset_fields_inherit_from_the_nearest_group_that_sets_them`, `a_chosen_terminal_type_reaches_the_server` |
| F-SSH-008 | Keepalive interval and dead-peer detection | Planned | ssh | — |
| F-SSH-009 | Auto-reconnect with backoff, keeping output and showing a separator. Per-host off switch, plus a manual Reconnect | In progress | ssh, session, app | `an_exit_is_told_apart_from_a_dropped_connection`, `a_dropped_connection_retries_then_gives_up_with_the_error`, `reconnects_back_off_up_to_half_a_minute`, `a_host_can_turn_off_reconnecting`, `a_background_tab_reconnects_when_its_time_comes`, `a_reconnect_keeps_the_earlier_output_above_a_line` |
| F-SSH-010 | Connection details panel: steps, negotiated algorithms, auth methods tried and offered | Planned | ssh, app | — |
| F-SSH-011 | **No hidden exec channels**: only what the user configured runs | In progress | ssh | `a_session_asks_the_server_for_nothing_the_user_did_not`, `channels_a_server_opens_unasked_are_refused` |
| F-SSH-012 | Environment variables on hosts and groups, inherited (SendEnv semantics) | In progress | ssh, vault, app | `unset_fields_inherit_from_the_nearest_group_that_sets_them`, `environment_lines_round_trip_and_bad_names_are_refused` |
| F-SSH-013 | Startup snippet runs once the shell is ready (host or group) | In progress | session, vault, app | `unset_fields_inherit_from_the_nearest_group_that_sets_them` |
| F-SSH-014 | Jump host chains: ordered, any length, inheritable. Errors name the failing hop | In progress | ssh, session, app | `a_host_behind_a_jump_host_is_reached_through_it`, `a_jump_chain_runs_outermost_first_and_stops_at_a_loop` |
| F-SSH-015 | Proxies: SOCKS5 and HTTP CONNECT (both with auth), and ProxyCommand | In progress | ssh | `proxy_urls_parse_with_defaults_and_logins`, `an_http_proxy_tunnels_and_a_refusal_is_reported`, `a_server_is_reached_through_a_socks_proxy` |
| F-SSH-016 | Agent forwarding per host, off by default | Planned | ssh | — |

## Port forwarding (F-FWD)

| ID | Feature | Status | Crate | Tests |
|---|---|---|---|---|
| F-FWD-001 | Local, remote and dynamic (SOCKS) rules: label, SSH host, bind address (127.0.0.1), ports, destination, description | In progress | ssh, vault | `local_and_socks_forwards_carry_bytes_and_a_busy_port_is_reported`, `a_remote_forward_reaches_a_destination_on_this_side`, `forwarding_rules_survive_lock_and_unlock_beside_the_hosts`, `a_rule_needs_a_name_ports_and_for_local_rules_a_destination`, `a_draft_rule_whose_id_was_taken_meanwhile_is_saved_under_the_id_it_answers`, `a_rule_open_to_the_network_says_so_on_its_row` |
| F-FWD-002 | Start/stop by toggle or double-click. Status (stopped, starting, active with connection count, error with reason) | In progress | ssh, app | `e2e_closing_terminals_sftp_and_forwards_leaves_nothing_on_the_server`, `e2e_a_local_rule_comes_back_after_its_connection_is_killed` |
| F-FWD-003 | Port-in-use detection with a clear error | Done | ssh | `local_and_socks_forwards_carry_bytes_and_a_busy_port_is_reported` |
| F-FWD-004 | Options to auto-start when the host connects and at app launch. Rules run without a terminal tab | In progress | ssh, app | `a_rule_starts_with_its_own_hosts_terminal_only_when_asked`, `auto_start_stays_on_the_device_that_set_it` |
| F-FWD-005 | Duplicate rule: a copy opens as a new rule named "… copy", with its port left to choose | In progress | app | `a_duplicate_is_a_new_rule_named_as_a_copy_that_listens_nowhere_yet`, `duplicating_a_rule_opens_a_new_one_that_saves_beside_it` |
| F-FWD-006 | A wizard for new rules: each kind drawn as a diagram a packet crosses (when shown, and again on hover), then three steps (what it does, server and ports, review with Save or Save and start). With no rules it is the screen | In progress | app | `each_kind_names_its_ends_and_says_what_it_does_with_what_was_typed`, `a_packet_moves_along_its_path_by_length`, `the_forwarding_wizard_saves_the_rule_it_describes`, `the_wizards_cards_are_reached_with_tab_and_picked_with_enter`, `the_wizards_suggested_name_follows_the_ports_until_one_is_typed`, `e2e_a_rule_made_in_the_wizard_carries_traffic_through_a_real_host` |

## SFTP (F-SFTP)

| ID | Feature | Status | Crate | Tests |
|---|---|---|---|---|
| F-SFTP-001 | Dual pane: local↔remote or remote↔remote, with a host picker per pane | In progress | sftp, app | `folders_come_first_and_dot_files_hide_unless_asked` |
| F-SFTP-002 | Navigation: editable path bar, refresh, up, home, hidden-files toggle, sort and filter | In progress | sftp, app | `listing_puts_directories_first_and_reports_sizes`, `path_helpers_handle_the_root_and_trailing_slashes`, `the_hosts_listing_sorts_by_its_headers_and_filters_by_name`, `listings_sort_by_any_column_with_folders_first`, `a_pending_folder_read_has_a_loading_state` |
| F-SFTP-003 | Operations: drag-drop between panes and from the OS, copy to the other pane, rename, delete (confirm), new folder, chmod dialog, properties | In progress | sftp, app | `folders_can_be_made_renamed_and_removed_with_their_contents`, `an_upload_lands_under_its_local_name_and_respects_conflicts`, `remote_names_that_would_escape_or_break_the_local_folder_are_refused`, `deleting_on_the_host_asks_first`, `e2e_the_new_sftp_tab_makes_a_folder_and_deletes_it_after_a_yes`, `a_rename_started_in_one_folder_ends_when_another_is_shown`, `a_file_dragged_onto_the_other_pane_is_uploaded_or_downloaded`, `files_dropped_from_the_os_upload_to_the_folder_shown`, `a_press_that_moves_far_is_a_drag_not_a_click`, `files_dropped_from_the_os_are_taken_once`, `a_folder_uploads_with_everything_in_it`, `a_folder_downloads_with_everything_in_it`, `a_right_click_on_a_file_opens_its_menu_in_either_pane`, `files_dragged_from_the_os_hover_until_they_leave_or_drop_where_said`, `a_drag_offers_the_panes_that_take_it`, `files_from_the_os_let_go_over_this_computer_are_copied_into_its_folder`, `this_computers_files_are_made_renamed_and_deleted_after_a_yes`, `changes_run_on_the_worker_and_refresh_the_cached_list` |
| F-SFTP-004 | Transfer queue: progress, speed, ETA, cancel, retry, resume | In progress | sftp | `a_download_copies_the_bytes_and_reports_progress`, `a_cancelled_download_leaves_no_partial_file`, `a_kept_part_of_the_same_file_is_continued_and_a_stale_one_is_not`, `an_upload_continues_a_kept_remote_part`, `the_pace_shows_speed_and_time_left_once_measurable` |
| F-SFTP-005 | Conflict prompt: overwrite, skip, keep both, apply to all. A taken name asks first by default (2026-11-07): Skip, Replace (a folder: Merge) or Rename under a name the user can change, and the answer can go for the others waiting | In progress | sftp, app | `a_download_never_replaces_a_local_file_unless_told_to`, `a_cancelled_overwrite_keeps_the_original_on_both_sides`, `a_name_taken_where_a_file_goes_asks_to_skip_replace_or_rename_it`, `one_answer_can_go_for_every_clash_waiting`, `a_transfer_renamed_by_the_user_lands_under_its_new_name_and_never_leaves_the_folder`, `local_files_with_the_same_name_wait_for_a_choice_and_both_are_kept`, `a_rejected_duplicate_copy_does_not_release_the_copy_already_using_that_name`, `replacing_a_local_file_keeps_the_old_one_until_the_new_copy_is_complete` |
| F-SFTP-006 | Open or edit a remote file in a local app (remembered per extension), upload on save | Planned | sftp, app | — |
| F-SFTP-007 | One-click switch between terminal and SFTP for the same host, following the cwd when the shell reports it (OSC 7) | In progress | app | — |
| F-SFTP-008 | Symlinks resolved and shown | In progress | sftp | — |
| F-SFTP-009 | File-manager columns in both panes: Size, Type, Modified, Permissions and Owner by default, Group, Accessed and Created on asking (a right click on the headers), kept in the settings; each sorts. Permissions read `rwxr-xr-x` on a host or a Unix computer, Read-only or Read/write on Windows; owners by name where the host lets anyone read `/etc/passwd` | In progress | sftp, app, config | `the_listings_show_permissions_and_a_right_click_on_the_headers_picks_columns`, `this_computers_files_say_what_they_allow_and_when_they_were_used`, `windows_attributes_read_as_words`, `the_host_names_its_owners_where_anyone_may_read_them`, `types_read_like_a_file_manager`, `columns_are_named_in_the_settings_by_their_keys` |

## Snippets (F-SNIP)

| ID | Feature | Status | Crate | Tests |
|---|---|---|---|---|
| F-SNIP-001 | Snippets (label + script) in packages, with drag into a package | In progress | vault, app | `search_needs_every_word_in_label_script_or_package`, `hosts_and_rules_reach_a_device_that_joins_and_edits_come_back` |
| F-SNIP-002 | Run in the active terminal, or paste without running | In progress | vault, app | `running_ends_with_enter_and_pasting_does_not` |
| F-SNIP-003 | Terminal side panel with snippet search | In progress | app | — |
| F-SNIP-004 | Run on multiple hosts: pick targets, open a workspace, prompt for missing credentials, saved target lists | In progress | app | `a_snippet_runs_after_the_hosts_own_startup_command`, `a_snippet_runs_on_the_hosts_ticked_in_one_tab_of_panes`, `several_hosts_share_one_tab_with_about_the_same_room_each` |
| F-SNIP-005 | Run in all panes of the current tab | In progress | app | `a_pane_left_out_of_broadcast_gets_no_typing` |

## Terminal (F-TERM)

| ID | Feature | Status | Crate | Tests |
|---|---|---|---|---|
| F-TERM-001 | VT/xterm emulation per ADR-0016: alt screen, scroll regions, mouse modes, bracketed paste, focus events, synchronized output. Mouse reports (clicks, drags, motion, wheel; SGR, UTF-8 and the original encoding) reach programs that ask for them; Shift keeps the mouse for selection | In progress | term, ui, app | `sgr_reports_one_based_cells_and_names_the_released_button`, `the_old_encoding_sends_offset_bytes_and_release_as_button_three`, `utf8_mode_writes_wide_coordinates_as_characters`, `the_wheel_presses_buttons_64_and_65_and_never_releases`, `motion_is_reported_only_in_the_modes_that_ask_for_it`, `button_events_show_each_press_and_release_even_within_one_frame`, `a_program_that_asked_for_the_mouse_gets_clicks_drags_and_releases`, `shift_keeps_the_mouse_for_selecting_text`, `a_click_reaches_a_program_that_turned_on_the_mouse` |
| F-TERM-002 | Truecolor, 256 colors, underline styles and colors | Planned | term | — |
| F-TERM-003 | Unicode: wide characters, emoji, combining marks, grapheme clusters. IME input | Planned | term, ui | — |
| F-TERM-004 | Scrollback: configurable (default 10,000 lines), with a memory cap | In progress | term, session, config, app | `the_scrollback_keeps_as_many_lines_as_asked_and_a_smaller_number_drops_the_oldest`, `settings_round_trip_and_bad_files_fall_back_to_defaults` |
| F-TERM-005 | Selection by character, word, line and block (Alt). Copy, copy-on-select option, right-click paste or menu | Planned | term, app | — |
| F-TERM-006 | Paste: bracketed, multi-line/large-paste warning, Shift+Insert. One line pastes without the line breaks after it, and a password the server asks for takes the keyboard, so Ctrl+Shift+V lands in it | In progress | app | `bracketed_paste_wraps_and_cannot_be_escaped`, `several_lines_are_confirmed_unless_the_program_brackets_pastes`, `a_paste_of_several_lines_waits_for_a_yes`, `one_line_pastes_without_the_line_breaks_after_it_and_several_keep_theirs`, `a_line_pasted_with_its_line_break_goes_in_as_one_line_without_asking`, `a_password_pasted_when_the_server_asks_for_one_reaches_its_field_whole` |
| F-TERM-007 | Find in scrollback: text, regex, case-sensitive, highlight all | In progress | term, app | `matches_on_screen_are_highlighted_and_steps_land_on_them`, `a_match_in_the_scrollback_is_scrolled_into_view`, `plain_text_is_escaped_and_case_is_explicit` |
| F-TERM-008 | URLs open with Ctrl/Cmd+click. OSC 8 links show their real URL (scheme allow-list) | In progress | term, app | `plain_urls_are_found_without_trailing_punctuation`, `a_url_wrapped_over_two_rows_is_one_link_from_either_row`, `every_link_on_screen_is_listed_once_in_order`, `osc8_links_report_their_real_target`, `only_web_and_mail_schemes_may_open`, `a_link_names_its_real_target_and_whether_a_click_opens_it` |
| F-TERM-009 | Cursor style (block/bar/underline), blink option, DECSCUSR | Planned | term | — |
| F-TERM-010 | Bell: visual, sound or none, plus a tab indicator | Planned | app | — |
| F-TERM-011 | OSC 52 clipboard write (off/ask/allow). Clipboard read is never allowed | Planned | term, app | — |
| F-TERM-012 | Fonts: independent app/terminal family selection, private TTF/OTF/TTC imports, bundled JetBrains Mono/Fira Mono and licensed Nerd symbol fallback ([ADR-0029](adr/0029-private-fonts-and-nerd-symbols.md)); line height, ligatures toggle and per-tab preferences remain | In progress | app, config, ui, platform | `bundled_terminal_fonts_and_symbols_have_the_expected_families`, `invalid_fonts_are_refused_before_any_private_file_is_written`, `a_valid_custom_font_is_copied_privately_and_can_be_loaded_again`, `nerd_symbols_shape_to_real_glyphs_with_both_bundled_terminal_families`, `changing_the_selected_family_changes_measurement_and_missing_fonts_fall_back`, `directwrite_has_real_bmp_and_supplementary_nerd_glyphs_without_os_installation`, `directwrite_font_changes_invalidate_format_and_layout_caches`, `app_and_terminal_font_choices_are_independent_and_remembered`, `glyphs_wider_than_a_cell_never_push_text_off_its_column`, `only_ascii_shares_a_run_and_every_other_glyph_keeps_its_own_column` |
| F-TERM-013 | Color themes: built-ins (our own plus open-licensed community schemes: the 498 tinted-theming schemes, [ADR-0019](adr/0019-tinted-color-schemes.md)), custom TOML, per-host/group override | In progress | term, config, app | `every_listed_theme_exists_and_its_text_is_readable`, `base24_dracula_maps_to_the_dracula_ansi_colors`, `base16_schemes_reuse_the_normal_colors_as_bright`, `every_scheme_keeps_terminal_text_readable`, `the_terminal_menu_lists_built_ins_first_and_each_name_once`, `terminal_names_resolve_to_built_ins_then_schemes`, `a_replayed_terminal_draws_in_the_chosen_theme` |
| F-TERM-014 | Clear screen and scrollback, reset terminal | In progress | term, app | `clearing_drops_the_scrollback_and_reset_also_the_screen_and_modes` |
| F-TERM-015 | Option-as-Meta (macOS), Alt sends ESC (Windows/Linux) | Planned | term | — |
| F-TERM-016 | Shell integration: OSC 7 cwd and OSC 133 prompt marks (jump to the previous/next prompt) | Planned | term | — |
| F-TERM-017 | Scroll bar: thumb sized to the screen's share of the scrollback, drag or press the track, ticks where an agent typed | In progress | term, session, app | `the_scroll_bar_thumb_is_the_screens_share_and_a_drag_lands_on_the_matching_row`, `rows_count_wrapped_pieces_and_a_row_scrolls_to_the_top_and_back` |

## Local shells (F-LOCAL)

| ID | Feature | Status | Crate | Tests |
|---|---|---|---|---|
| F-LOCAL-001 | Open a local terminal with the default profile, a full xterm environment and the user's home folder | In progress | session, platform, app | `a_shell_runs_and_its_output_reaches_the_screen`, `local_programs_get_a_full_terminal_environment`, `local_programs_start_in_the_users_home_folder`, `editing_and_function_keys_use_xterm_sequences`, `a_real_shell_runs_in_the_new_terminal_and_gets_what_is_typed`, `keys_text_and_pastes_become_the_bytes_a_program_expects`, `named_editing_keys_never_type_attached_text` |
| F-LOCAL-002 | Auto-detected profiles. Windows: PowerShell 7, Windows PowerShell, cmd, WSL distros, Git Bash. macOS/Linux: login shell plus `/etc/shells` | In progress | session, app | `wsl_output_is_utf16_with_a_byte_order_mark_and_blank_lines`, `etc_shells_skips_comments_and_repeats`, `something_is_always_offered`, `a_console_title_that_only_names_the_shells_file_leaves_the_tab_its_name` |
| F-LOCAL-003 | Custom profiles (path, args, cwd, env, icon) and a default-profile setting | Planned | config | — |

## Split view and workspaces (F-WS)

| ID | Feature | Status | Crate | Tests |
|---|---|---|---|---|
| F-WS-001 | Split right/down, up to 16 panes per tab, drag dividers, drag panes to reposition, drag a tab onto a tab, move a pane to its own tab. A split glides the panes and divider into place and slides in the pane bar | In progress | app, ui | `splitting_focuses_the_new_pane_and_closing_keeps_one`, `parts_fill_the_area_with_gaps`, `a_new_split_glides_in_with_its_bar_and_settles` |
| F-WS-002 | Focus mode (one large pane plus a side list) vs Split mode | In progress | app | `focus_mode_gives_the_focused_pane_the_room_beside_a_list`, `focus_mode_and_broadcast_are_a_click_or_a_shortcut_away` |
| F-WS-003 | Broadcast input to all or selected panes in a tab, with an obvious indicator and per-pane exclusion | In progress | session, app | `broadcast_needs_two_panes_and_ends_when_one_is_left`, `focus_mode_and_broadcast_are_a_click_or_a_shortcut_away` |
| F-WS-004 | Saved workspaces (tabs, layout, hosts/profiles, per-pane cwd), synced. An unsaved-changes dot, Ctrl/Cmd+S to save | Planned | vault, app | — |
| F-WS-005 | Pane navigation and resize shortcuts | In progress | app | `focus_moves_round_and_a_tab_holds_at_most_sixteen` |

## Session logs (F-LOG)

| ID | Feature | Status | Crate | Tests |
|---|---|---|---|---|
| F-LOG-001 | Record output per host, group or globally (off by default, since output can contain secrets) | In progress | session | `the_transcript_includes_scrollback_and_drops_colors`, `a_log_of_a_tab_closed_while_locked_is_saved_once_the_vault_opens` |
| F-LOG-002 | Logs screen: by date, filter by host, bookmarked first. Open as a read-only tab with search | In progress | app | — |
| F-LOG-003 | Retention: last N per host (default 5), bookmark with comment to keep, delete one or all | In progress | vault, app | `each_title_keeps_its_five_newest`, `a_log_is_deleted_only_after_a_yes` |
| F-LOG-004 | Export a log as plain text | In progress | app | `exports_get_a_safe_unique_name_and_never_overwrite` |
| F-LOG-005 | Logs encrypted and synced like any record, with a per-log size cap and a truncation notice | In progress | vault | `long_logs_keep_their_end` |

## Autocomplete (F-AC)

| ID | Feature | Status | Crate | Tests |
|---|---|---|---|---|
| F-AC-001 | Suggestion popup from local history (per host and global) and snippets. Accept with Tab/→, dismiss with Esc, never auto-run | Planned | app | — |
| F-AC-002 | Works in PowerShell, cmd, bash, zsh and fish, local and remote | Planned | app, term | — |
| F-AC-003 | Optional remote path completion via an SFTP listing | Planned | sftp, app | — |
| F-AC-004 | Toggle globally, per host, and "pause for this session" | Planned | app | — |
| F-AC-005 | History stored encrypted in the vault, synced (setting), clearable | Planned | vault | — |
| F-AC-006 | At a detected password prompt, offer a saved password to type. Explicit pick only | Planned | app | — |

## Account and sync (F-SYNC)

| ID | Feature | Status | Crate | Tests |
|---|---|---|---|---|
| F-SYNC-001 | Local mode: the full app with no account | In progress | vault | `hosts_survive_lock_and_unlock_and_never_appear_in_the_clear`, `a_vault_from_an_earlier_build_opens_with_everything_in_it` |
| F-SYNC-002 | Create an account on a self-hosted server: URL, username, master password with strength meter, recovery key shown and confirmed | In progress | sync, app | `a_full_round_over_http_then_a_revoked_device_is_refused`, `common_passwords_and_their_disguises_are_weak`, `repeats_sequences_rows_and_dates_are_seen_through`, `random_text_and_passphrases_are_strong`, `a_long_but_common_master_password_is_refused`, `a_key_file_asking_for_too_much_memory_is_refused_before_any_work` |
| F-SYNC-003 | Sign in on another device. A device with existing local data chooses merge, replace (after export) or keep separate. The account card fits the status shown in its current frame | In progress | sync, app | `hosts_and_rules_reach_a_device_that_joins_and_edits_come_back`, `adopted_entities_get_new_ids_and_rules_follow_their_hosts`, `the_account_card_sizes_its_surface_for_the_status_drawn_this_frame` |
| F-SYNC-004 | Continuous sync (SSE), offline queue, sync status in the title bar with last-synced time | In progress | sync | `a_full_round_over_http_then_a_revoked_device_is_refused`, `the_pill_says_synced_waiting_or_failed`, `plain_http_is_only_for_this_computer`, `rounds_come_a_minute_apart_sooner_after_an_edit_or_a_failure`, `connecting_or_unlocking_brings_the_accounts_latest_at_once` |
| F-SYNC-005 | Conflicts: newer version wins, notify, compare and restore the other version | In progress | sync, app | `the_newer_of_two_offline_edits_wins_on_both_devices`, `a_losing_local_edit_is_kept_in_the_server_history`, `an_edit_beats_a_deletion_from_another_device`, `an_old_version_served_again_is_not_applied_and_the_newer_goes_back`, `a_deleted_record_is_not_brought_back_by_its_last_version` |
| F-SYNC-006 | Version history per item, with restore | In progress | vault, server | `history_keeps_the_last_twenty_versions` |
| F-SYNC-007 | Trash for 30 days, with restore | In progress | vault, server, sync, app | `a_deleted_host_comes_back_from_the_trash_on_every_device`, `a_deletion_the_server_made_up_is_passed_over` |
| F-SYNC-008 | Device list (name, OS, last seen), rename, revoke | In progress | sync, server | `a_revoked_device_is_signed_out_at_once`, `a_device_token_unused_for_ninety_days_stops_working` |
| F-SYNC-009 | Change master password; regenerate recovery key | In progress | vault, sync, app | `a_password_change_off_the_ui_thread_needs_the_current_password`, `a_full_round_over_http_then_a_revoked_device_is_refused`, `signing_in_again_on_the_same_vault_relinks_without_duplicates`, `a_new_password_must_be_long_strong_and_typed_twice`, `a_password_change_needs_the_current_password_not_just_a_device_token`, `a_login_proves_only_what_and_whom_it_was_started_for` |
| F-SYNC-010 | Forgot password: recover with the recovery key | Planned | vault, sync | — |
| F-SYNC-011 | Encrypted export/import file (to local mode or any server) | Planned | vault | — |
| F-SYNC-012 | Sign out keeps the local vault. "Remove from this device" is explicit and offers an export | In progress | app | `a_snapshot_older_than_a_local_edit_is_not_applied` |
| F-SYNC-013 | Option to keep private keys and passwords device-local (not synced) | Planned | sync | — |
| F-SYNC-014 | Optional TOTP second factor for server sign-in | Planned | server, app | — |

## Sync server (F-SRV)

| ID | Feature | Status | Crate | Tests |
|---|---|---|---|---|
| F-SRV-001 | Static binaries (x86-64/arm64, all OSes), a multi-arch Docker image, a compose example. Its own zip per OS, never in the client's downloads ([ADR-0027](adr/0027-client-and-server-ship-apart.md)) | In progress | server, xtask | `the_client_and_server_are_written_apart_with_every_folder_named` |
| F-SRV-002 | TOML/env config. Built-in TLS, or `--behind-proxy`; refuses plain HTTP off loopback. Where it listens: `--host` and `--port` (2026-11-07) or `--listen` with both; `--help` lists every option | In progress | server | `plain_http_beyond_loopback_needs_tls_or_a_proxy`, `host_and_port_set_where_it_listens_in_any_order`, `trusted_proxy_addresses_need_proxy_mode`, `forwarded_addresses_are_trusted_only_from_a_local_proxy`, `a_client_cannot_pick_its_address_by_sending_its_own_forwarded_header`, `store_work_runs_off_the_async_request_thread`, `a_servers_own_certificate_is_trusted_only_once_pinned_and_only_that_one`, `a_new_certificate_is_written_once_with_a_private_key_and_its_fingerprint`, `trusting_a_servers_certificate_pins_it_until_the_address_changes` |
| F-SRV-003 | Multiple isolated accounts; sign-ups closed/invite/open | In progress | server | `accounts_never_see_each_others_records`, `closed_signups_and_taken_names_are_refused`, `closed_signups_take_a_single_use_unexpired_invite`, `closed_sign_ups_take_an_invite_code_over_http` |
| F-SRV-004 | Admin CLI: create the first account, invites, list/disable/delete accounts, storage use | In progress | server | `admin_commands_take_their_argument`, `the_admin_sees_sizes_deletes_accounts_and_takes_backups`, `an_invite_lasts_a_day_unless_told_otherwise_and_never_beyond_thirty` |
| F-SRV-005 | Online backup and restore | In progress | server | `the_admin_sees_sizes_deletes_accounts_and_takes_backups` |
| F-SRV-006 | Rate limiting, generic auth errors, request size limits | In progress | server | `wrong_passwords_and_unknown_accounts_fail_the_same_way_then_throttle`, `only_a_signed_in_push_may_be_large_and_strangers_are_refused_first`, `a_first_sync_with_several_large_records_goes_through_over_http`, `account_names_are_checked_before_anything_is_kept`, `a_full_failure_table_makes_room_for_a_new_guesser`, `a_full_login_table_drops_the_oldest_login`, `a_client_cannot_pick_its_address_by_sending_its_own_forwarded_header`, `a_push_of_too_many_changes_is_refused_whole`, `a_push_never_holds_more_changes_than_the_server_takes` |
| F-SRV-007 | Health endpoint. Logs without bodies, tokens or ids | In progress | server, sync | `the_health_check_answers_anyone_without_a_version_or_a_token` |
| F-SRV-008 | Admin guide: install, TLS, backup, upgrade (`docs/server/`) | In progress | docs | — |
| F-SRV-009 | Protocol versioning: the server accepts the current and previous client protocol | In progress | proto, server | `messages_survive_encoding`, `garbage_is_an_error_not_a_panic` |

## App lock and security (F-SEC)

| ID | Feature | Status | Crate | Tests |
|---|---|---|---|---|
| F-SEC-001 | Master password unlock | In progress | vault, app | `the_password_unlocks_and_a_wrong_one_is_reported_as_such`, `the_recovery_key_unlocks_after_a_forgotten_password`, `password_work_has_a_live_inert_unlock_state`, `password_work_renders_in_each_theme_and_renderer`, `reduced_motion_keeps_password_work_still`, `the_vault_opens_as_the_lock_leaves_and_the_leaving_lock_takes_nothing`, `a_record_that_does_not_read_is_reported_without_its_text`, `what_the_lock_screen_hands_over_never_prints_the_secret`, `a_write_replaces_the_file_keeps_the_last_one_and_leaves_no_temporary`, `a_failed_write_leaves_the_old_file_alone`, `a_second_window_cannot_open_the_same_data_folder` |
| F-SEC-002 | PIN quick unlock (6+ digits, 5 tries, needs the device secret store) | Planned | vault, platform | — |
| F-SEC-003 | Biometric quick unlock: Windows Hello, Touch ID (not available on Linux) | Planned | platform | — |
| F-SEC-004 | Auto-lock on idle (default 15 min; never, or 5 min up to 8 h), on OS lock and on sleep, plus a "Lock now" command | In progress | app | `idle_time_at_or_past_the_timeout_locks`, `locking_drops_the_unlocked_hosts`, `the_lock_screens_tree_names_the_password_fields_but_never_their_text` |
| F-SEC-005 | Copied secrets auto-clear from the clipboard after 30 s | In progress | app, platform | `a_copied_secret_is_cleared_after_its_time_unless_something_else_was_copied`, `first_run_creates_the_vault_and_opens_it_only_after_the_recovery_key_is_confirmed` |
| F-SEC-006 | Secrets never appear in logs (enforced by test) | In progress | all | `secrets_never_show_in_debug_output`, `keys_and_group_proxies_never_show_in_debug_output`, `a_saved_password_never_shows_in_debug_output` |

## Import and export (F-IMP)

| ID | Feature | Status | Crate | Tests |
|---|---|---|---|---|
| F-IMP-001 | Import OpenSSH config (Include, ProxyJump → chain, IdentityFile → keys) with a preview | In progress | import, app | `the_first_value_wins_and_wildcards_give_defaults`, `includes_negations_and_match_blocks`, `forward_lines_become_rules` |
| F-IMP-002 | Import PuTTY sessions (registry on Windows, `.reg` elsewhere), including PPK keys and tunnels | Planned | import | — |
| F-IMP-003 | Export hosts as `ssh_config` (no secrets), with a warning | In progress | import, app | `export_writes_no_secrets_and_round_trips` |

## Settings (F-SET)

| ID | Feature | Status | Crate | Tests |
|---|---|---|---|---|
| F-SET-001 | Settings screen generated from the schema (sections in UI-SPEC) | In progress | app, config | `settings_round_trip_and_bad_files_fall_back_to_defaults`, `every_listed_chord_is_one_of_a_kind_and_the_app_checks_for_it`, `chords_are_written_the_way_the_keyboard_says_them`, `a_settings_write_failure_is_shown_instead_of_silently_losing_the_change` |
| F-SET-002 | App theme: light, dark or system, with a searchable family browser, variant filters, recent themes and live preview on pointer movement ([ADR-0028](adr/0028-theme-browser.md)); all 498 community schemes and six built-in palettes for the whole window ([ADR-0019](adr/0019-tinted-color-schemes.md)); the Light and Dark tabs list only schemes of their kind ([ADR-0026](adr/0026-the-theme-mode-filters-the-color-schemes.md)) | In progress | ui, config, app | `both_themes_meet_wcag_aa`, `system_mode_follows_the_os_and_fixed_modes_ignore_it`, `every_scheme_meets_wcag_aa`, `both_themes_keep_text_readable_on_every_surface`, `our_light_theme_keeps_white_cards_menus_and_fields`, `a_scheme_recolors_the_window_and_none_keeps_our_own_theme`, `the_catalog_holds_every_scheme_once_in_menu_order`, `a_step_reaches_its_contrast_and_no_further`, `the_keyboard_reaches_every_item_of_a_long_menu_and_enter_picks_it`, `the_wheel_scrolls_an_open_menu_and_not_the_page_under_it`, `a_long_menu_searches_as_you_type_enter_picks_and_hover_is_reported`, `the_hovered_scheme_shows_until_the_menu_lets_go_and_each_entry_has_a_swatch`, `a_menus_scroll_bar_drags_and_jumps_and_keeps_the_menu_open`, `new_colors_blend_in_and_reduced_motion_shows_them_at_once`, `a_mode_of_the_other_kind_drops_the_scheme_and_system_keeps_any`, `the_theme_tabs_offer_schemes_of_their_kind_and_drop_one_of_the_other`, `the_theme_browser_previews_cancels_and_applies_without_touching_background_controls`, `the_browser_contains_every_scheme_once_and_keeps_families_together`, `search_and_variant_filters_intersect_without_changing_preferences`, `the_six_built_in_palettes_resolve_and_have_readable_palettes`, `the_theme_preview_uses_the_selected_schemes_actual_terminal_palette`, `hovering_a_theme_previews_it_on_the_first_frame` |
| F-SET-003 | Settings file hot reload with validation and readable errors | Planned | config | — |
| F-SET-004 | Keymap editor: edit, reset, conflict detection, an alternative preset | In progress | app, config | `keys_chosen_in_settings_replace_a_shortcuts_own_until_they_are_dropped`, `every_chord_reads_back_from_how_it_is_written`, `keys_that_would_take_typing_or_belong_to_another_are_refused`, `a_shortcut_takes_new_keys_in_settings_and_its_own_again_on_reset`, `a_reset_that_would_share_keys_is_refused_and_reset_all_puts_everything_back`, `no_screen_writes_out_the_keys_of_a_shortcut_that_can_change`, `on_a_mac_keys_must_hold_cmd_and_leave_ctrl_letters_to_the_shell`, `settings_round_trip_and_bad_files_fall_back_to_defaults` |
| F-SET-005 | Debug log toggle | Planned | app | — |
| F-SET-006 | Opt-in, signed update check | Planned | app | — |
| F-SET-007 | Portable mode | Planned | app | — |

## Platform (F-PLAT)

| ID | Feature | Status | Crate | Tests |
|---|---|---|---|---|
| F-PLAT-001 | Windows: custom title bar with snap layouts. A portable zip of the app and `annulus-agent`, and our own installer (per user, no administrator; Start menu, desktop and PATH as chosen; removed from Apps & features), [ADR-0027](adr/0027-client-and-server-ship-apart.md); MSIX and winget later | In progress | platform, setup, xtask | `installing_puts_the_programs_shortcuts_path_and_entry_in_place_and_removing_takes_them`, `a_damaged_or_empty_installer_refuses_before_writing_anything`, `the_path_gains_the_folder_once_and_loses_only_it`, `programs_go_where_windows_puts_one_users_programs`, `the_first_page_offers_the_folder_and_what_to_add_and_cancel_closes`, `a_folder_that_is_not_a_full_path_is_not_installed_to`, `a_zip_holds_each_file_whole_in_its_folder_with_its_mode`, `registry_strings_are_utf16_with_their_nul` |
| F-PLAT-002 | macOS: native traffic lights, app menu, Cmd shortcuts, signed and notarized `.dmg` | In progress | platform, app, ui, xtask | `on_macos_the_title_bar_leaves_the_window_buttons_to_the_os`, `the_command_key_and_v_paste_in_a_text_field_and_on_macos_anywhere`, `select_all_and_copy_hand_text_out_but_never_from_a_secret`, `on_a_mac_keys_must_hold_cmd_and_leave_ctrl_letters_to_the_shell`, `chords_are_written_the_way_the_keyboard_says_them`, `every_listed_chord_is_one_of_a_kind_and_the_app_checks_for_it`, `a_chord_names_the_key_pressed_not_the_character_shift_makes`, `an_unidentified_mac_delete_uses_its_physical_backspace_key`, `named_editing_keys_never_type_attached_text`, `on_a_mac_a_ctrl_click_is_a_right_click_from_press_to_release`, `keys_text_and_pastes_become_the_bytes_a_program_expects`, `the_menu_bars_items_do_what_their_shortcuts_do_and_nothing_while_locked`, `the_menu_bar_shows_the_shortcuts_keys_but_none_while_settings_records_keys`, `a_menu_item_shows_a_chords_key_and_its_modifiers_as_appkit_spells_them`, `the_bundle_holds_the_binary_its_icon_and_an_info_plist_naming_both`, `the_macos_icon_file_holds_each_image_under_its_type`, `on_macos_the_tile_sits_centered_with_a_clear_margin_around_it` |
| F-PLAT-003 | Linux: Wayland and X11, fractional scaling, AppImage + `.deb` | Planned | platform | — |
| F-PLAT-004 | HiDPI and per-monitor DPI | Planned | platform, ui | — |
| F-PLAT-005 | Accessibility via AccessKit on all three OSes | In progress | ui, platform, app | `a_rows_buttons_are_its_children_and_a_menu_above_a_backdrop_is_the_windows`, `a_secret_field_is_a_password_input_without_its_text`, `requests_map_back_to_zones_and_unknown_actions_are_ignored`, `a_screen_readers_click_and_focus_work_like_the_keyboards`, `the_lock_screens_tree_names_the_password_fields_but_never_their_text`, `two_terminals_with_the_same_cards_open_share_no_zone_id`, `a_list_clip_has_room_for_rings_at_its_sides_and_at_the_end_it_is_scrolled_to`, `a_ring_cut_off_by_a_plain_clip_is_reported_and_a_list_clip_keeps_it_whole` |

## Automation: CLI and AI agents (F-AUTO)

Added 2026-10-31 ([ADR-0020](adr/0020-local-control-channel-for-cli-and-ai.md)). Off until switched on in Settings.

| ID | Feature | Status | Crate | Tests |
|---|---|---|---|---|
| F-AUTO-001 | A local control door, off by default, open only while switched on and unlocked, token first | In progress | control, app | `the_door_is_open_only_while_switched_on_and_unlocked`, `a_client_with_the_token_is_answered_and_the_door_file_goes_when_it_closes`, `a_wrong_token_gets_nothing_back_and_no_request_reaches_the_app`, `calls_and_replies_read_back_as_written_with_the_op_and_ok_names_on_the_wire` |
| F-AUTO-002 | `annulus-agent` CLI (named so since [ADR-0027](adr/0027-client-and-server-ship-apart.md)): status, hosts, tabs, read; never a secret in any answer | In progress | cli, control, app | `no_password_proxy_login_or_key_of_the_vault_is_in_any_answer`, `hosts_tabs_and_terminal_text_are_answered_as_the_window_shows_them`, `the_command_line_reads_each_command_and_refuses_what_it_does_not_know`, `tables_show_a_port_only_when_it_is_not_22_and_mark_the_tab_in_front_and_the_agents`, `what_is_printed_cannot_drive_the_terminal` |
| F-AUTO-003 | Agents open their own tabs (marked AI), run commands in them (answered with how far each got: done once its prompt is back, waiting, running, interrupted or ended), wait for one still going and interrupt them (Ctrl+C); the user's own tabs are read-only to them | In progress | app, control, cli | `an_agent_opens_its_own_tab_and_may_type_into_and_close_only_that`, `an_agent_runs_a_command_through_the_door_in_its_own_tab_and_gets_its_output`, `an_agents_command_runs_out_of_its_wait_is_interrupted_and_stop_cuts_off_the_next`, `an_agent_hears_why_its_tab_could_not_connect_and_its_command_is_refused_at_once`, `an_agents_silent_command_is_not_done_until_its_prompt_is_back_and_wait_follows_it`, `a_commands_output_starts_at_the_line_it_was_typed_on`, `a_commands_output_is_found_by_its_line_once_the_full_scrollback_drops_lines`, `a_prompt_is_back_when_the_last_line_is_one_like_it_even_in_another_folder`, `a_command_is_done_once_its_prompt_is_back_and_a_silent_one_never_by_quiet_alone`, `a_command_is_typed_once_the_shell_has_shown_text_and_settled`, `a_command_typed_before_the_prompt_ends_at_the_prompt_it_was_echoed_at`, `a_commands_words_in_its_output_are_no_echo_when_the_echo_is_lost`, `a_command_wrapped_at_the_terminals_width_is_no_output_of_its_own`, `an_exit_from_ssh_is_done_at_the_local_prompt_and_settles_the_ssh_before_it`, `a_finished_command_prints_its_output_and_an_unfinished_one_says_why`, `past_eight_clients_the_newest_is_turned_away_until_one_leaves` |
| F-AUTO-004 | The agent panel beside its tab (in memory only): each command's state, time, run time and lines; picking one shows it in the terminal, whose margin marks the agent's lines; folds to a rail; Stop ends all access | In progress | app, term, session | `an_agent_runs_a_command_through_the_door_in_its_own_tab_and_gets_its_output`, `a_finished_command_counts_its_output_and_a_timed_out_one_keeps_its_lines_so_far`, `each_command_says_when_in_the_session_how_long_and_how_much`, `a_long_command_wraps_to_three_lines_and_the_rest_is_cut_short`, `the_margin_marks_every_command_and_tints_the_picked_one_up_to_the_next`, `rows_count_wrapped_pieces_and_a_row_scrolls_to_the_top_and_back`, `an_agents_command_runs_out_of_its_wait_is_interrupted_and_stop_cuts_off_the_next`, `the_folded_agent_rail_is_a_card_clear_of_the_window_edge_and_its_tally_opens_the_panel` |
| F-AUTO-005 | `annulus-agent mcp`: the same operations as MCP tools over stdio, calls side by side | In progress | cli | `an_mcp_client_initializes_lists_the_tools_and_calls_them`, `a_line_that_is_not_json_gets_a_parse_error_and_the_server_goes_on`, `tool_calls_run_side_by_side_so_a_long_run_holds_back_no_interrupt` |
| F-AUTO-006 | Automation settings: agents' tabs open in front or behind (started unseen), agents may read the user's tabs or only their own, agents may not close tabs (on by default; while on, a tab whose shell ended stays too), else an agent's tab may close once its shell ends, the agent panel starts folded | In progress | app, config | `agents_tabs_open_behind_and_folded_and_the_owners_tabs_close_to_them_when_set`, `an_agents_tab_opened_behind_starts_unseen_and_closes_once_its_shell_ends`, `an_agents_tab_stays_open_until_the_owner_lets_agents_close_tabs`, `an_ended_agents_tab_closes_only_while_agents_may_close_tabs`, `settings_round_trip_and_bad_files_fall_back_to_defaults` |
| F-AUTO-007 | Agents settings page and command approval: Security and Agents share one access switch; approval is off by default and, when on, an agent's `run` waits with no terminal input until the user approves or denies its exact command in the agent panel | In progress | app, config | `security_and_agents_share_one_agent_access_switch`, `command_approval_holds_the_agent_exe_until_the_owner_approves_or_denies`, `settings_round_trip_and_bad_files_fall_back_to_defaults` |
| F-AUTO-008 | One-click install of Annulus's portable Agent Skill for Claude Code, Codex, Pi, Hermes, Gemini CLI, Cursor, OpenCode and GitHub Copilot | In progress | app | `the_annulus_skill_installs_for_named_and_shared_agent_harnesses` |
| F-AUTO-009 | `annulus-agent scp` and MCP uploads/downloads through saved hosts over SFTP; visible AI file tabs, approval, cancellation, opt-in overwrite, directory copies and safe temporary-file cleanup advice (ADR-0032) | In progress | cli, control, app, sftp | `scp_keeps_spaces_and_shell_text_as_paths_in_both_directions`, `scp_output_includes_safe_cleanup_advice_in_text_and_json`, `agent_copies_keep_exact_names_bytes_and_existing_files`, `agent_directory_copies_refuse_existing_targets_in_both_directions`, `agent_transfer_approval_and_stop_hold_and_cancel_file_writes`, `transfer_approval_buttons_and_stop_control_the_copy` |

## Out of scope

Decided 2026-10-28. "Later" means it may be reconsidered after 1.0, only if the maintainer adds it here.

| Feature | Reason | Later? |
|---|---|---|
| Teams: shared/team vaults, roles, invites, sharing hosts or credentials, seats | Not planned: no team features | No |
| SSO (SAML/OIDC), SCIM, audit logs, admin console, SOC 2 | Team or enterprise feature | No |
| Multiplayer / live terminal sharing | Team feature | No |
| Mobile apps (iOS, Android) and mobile-only UI (extra keys, gestures, screenshot blur) | Not planned: desktop only | No |
| Telnet, Serial, Mosh | Not planned: SSH + local only ([ADR-0011](adr/0011-mosh-telnet-serial.md)) | Maybe |
| AI command or snippet generation | Not planned: not wanted | No |
| Cloud host sync (AWS, DigitalOcean, Azure) | Not planned | No |
| Folder-based sync (OneDrive, Syncthing) | A self-hosted server was chosen instead | No |
| Several vaults per account | One vault keeps it simple | Maybe |
| MobaXterm, SecureCRT, CSV import | Not needed for v1 | Maybe |
| ML-DSA keys, standalone ML-KEM KEX | No russh or mainstream server support yet | Maybe |
| Keyword highlighting in output | Extra | Maybe |
| Snippet variables/placeholders | Extra | Maybe |
| Multiple app windows | Extra | Maybe |
| Password breach check (haveibeenpwned) | Calls a third party | No |
| Inline images (sixel/kitty), kitty keyboard protocol | Extra | Maybe |
| Port knocking | Rarely used | No |
