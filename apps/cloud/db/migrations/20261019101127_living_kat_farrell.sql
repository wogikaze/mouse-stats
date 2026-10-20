ALTER TABLE `devices` ADD `recovery_key` text DEFAULT '' NOT NULL;--> statement-breakpoint
ALTER TABLE `device_sessions` ADD `previous_refresh_hash` text;--> statement-breakpoint
ALTER TABLE `device_sessions` ADD `rotation_nonce_hash` text;