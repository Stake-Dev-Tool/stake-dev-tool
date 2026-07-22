-- Surface a scheduled cancellation. When a subscriber cancels through the Stripe
-- Customer Portal, Stripe keeps the subscription `active` but flips
-- `cancel_at_period_end` to true (access runs to the period end, then it lapses).
-- Persist that flag from the webhook so the billing page can show a calm
-- "your plan ends on <date>" notice. Defaults to false for existing rows.
ALTER TABLE subscriptions ADD COLUMN cancel_at_period_end BOOLEAN NOT NULL DEFAULT false;
