-- Single seat-based plan. The two-plan model (solo/team) collapses into one
-- `'paid'` plan whose quotas scale by a per-workspace seat count. Subscriptions
-- gain a `seats` column (the Stripe quantity); plan overrides gain a nullable
-- `seats` (required for a 'paid' comp, NULL for 'unlimited'). Both plan CHECK
-- constraints are re-narrowed to the new label set.

-- subscriptions: add seats, migrate solo/team → paid (mapping team → 10 seats,
-- solo → 1), then narrow the plan CHECK to just 'paid'. Storage-only placeholder
-- rows (plan was 'solo') fold into 'paid' too — they stay non-plan-granting via
-- their status, and 'paid' is simply the CHECK-valid placeholder.
ALTER TABLE subscriptions ADD COLUMN seats INTEGER NOT NULL DEFAULT 1;
ALTER TABLE subscriptions DROP CONSTRAINT subscriptions_plan_check;
UPDATE subscriptions SET seats = CASE WHEN plan = 'team' THEN 10 ELSE 1 END
  WHERE plan IN ('solo', 'team');
UPDATE subscriptions SET plan = 'paid' WHERE plan IN ('solo', 'team');
ALTER TABLE subscriptions ADD CONSTRAINT subscriptions_plan_check CHECK (plan = 'paid');

-- plan_overrides: add nullable seats, migrate solo → paid/1, team → paid/10,
-- leave 'unlimited' as-is (seats NULL), then narrow the plan CHECK.
ALTER TABLE plan_overrides ADD COLUMN seats INTEGER;
ALTER TABLE plan_overrides DROP CONSTRAINT plan_overrides_plan_check;
UPDATE plan_overrides SET seats = CASE WHEN plan = 'team' THEN 10 WHEN plan = 'solo' THEN 1 END
  WHERE plan IN ('solo', 'team');
UPDATE plan_overrides SET plan = 'paid' WHERE plan IN ('solo', 'team');
ALTER TABLE plan_overrides ADD CONSTRAINT plan_overrides_plan_check CHECK (plan IN ('paid', 'unlimited'));
