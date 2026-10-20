-- void:allow-destructive
-- This rebuild copies every hourly row before replacing the table to add validation constraints.
PRAGMA foreign_keys=OFF;--> statement-breakpoint
CREATE TABLE `__new_hourly` (
	`recording_id` text NOT NULL,
	`hour_start` integer NOT NULL,
	`key_code` integer NOT NULL,
	`value` integer NOT NULL,
	PRIMARY KEY(`recording_id`, `hour_start`, `key_code`),
	FOREIGN KEY (`recording_id`) REFERENCES `recordings`(`id`) ON UPDATE no action ON DELETE cascade,
	CONSTRAINT "hourly_value_range" CHECK("__new_hourly"."value">=0 AND "__new_hourly"."value"<=9007199254740991),
	CONSTRAINT "hourly_utc" CHECK("__new_hourly"."hour_start">=0 AND "__new_hourly"."hour_start"%3600=0),
	CONSTRAINT "hourly_physical_key" CHECK("__new_hourly"."key_code" BETWEEN -1 AND 127 OR "__new_hourly"."key_code" BETWEEN 256 AND 383 OR "__new_hourly"."key_code" BETWEEN 512 AND 639)
);
--> statement-breakpoint
INSERT INTO `__new_hourly`("recording_id", "hour_start", "key_code", "value") SELECT "recording_id", "hour_start", "key_code", "value" FROM `hourly`;--> statement-breakpoint
DROP TABLE `hourly`;--> statement-breakpoint
ALTER TABLE `__new_hourly` RENAME TO `hourly`;--> statement-breakpoint
PRAGMA foreign_keys=ON;--> statement-breakpoint
CREATE INDEX `hourly_hour` ON `hourly` (`hour_start`);
