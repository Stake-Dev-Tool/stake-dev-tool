-- M7 provider swap: Polar → Stripe. The subscriptions table is provider-neutral
-- from here on, so the two Polar-specific id columns are renamed to generic
-- `provider_*` names (the values are now Stripe subscription/customer ids). A new
-- `extra_storage_units` column backs the quantity-based storage add-on: one unit
-- grants +10 GiB on top of the plan's storage cap.
--
-- Storage is modelled on the SAME single-row-per-workspace table. A Stripe
-- subscription that carries only the storage price (bought separately from the
-- plan) upserts this row: when a plan row already exists it touches only
-- `extra_storage_units`; when none exists it inserts a placeholder with
-- plan='solo', interval='monthly', status='storage_only'. `plan_for` treats the
-- 'storage_only' status as NOT plan-granting (the workspace falls through to its
-- trial), while `limits_for` still adds the storage on top.

ALTER TABLE subscriptions RENAME COLUMN polar_subscription_id TO provider_subscription_id;
ALTER TABLE subscriptions RENAME COLUMN polar_customer_id TO provider_customer_id;
ALTER TABLE subscriptions ADD COLUMN extra_storage_units INTEGER NOT NULL DEFAULT 0;
