# Configuration Recipes: Codex Routing Graph

This file is the user-facing recipe book for the v4 routing graph.

## The Shape

The target shape is:

```toml
[codex.routing]
entry = "main_route"

[codex.routing.routes.main_route]
strategy = "ordered-failover"
children = ["monthly_pool", "codex_for"]
```

`children` may reference providers or other routes. A monthly pool is just a named route node.

## 1. Single Provider

Use this when you only need one relay and do not want any hidden fallback behavior.

```toml
version = 4

[codex.providers.main]
base_url = "https://api.example.com/v1"
auth_token_env = "MAIN_API_KEY"

[codex.routing]
entry = "main_route"

[codex.routing.routes.main_route]
strategy = "manual-sticky"
target = "main"
```

## 2. Ordered Failover

Use this when you want a simple first-choice / second-choice chain.

```toml
version = 4

[codex.providers.input]
base_url = "https://input.example/v1"
auth_token_env = "INPUT_API_KEY"

[codex.providers.backup]
base_url = "https://backup.example/v1"
auth_token_env = "BACKUP_API_KEY"

[codex.routing]
entry = "main_route"

[codex.routing.routes.main_route]
strategy = "ordered-failover"
children = ["input", "backup"]
```

## 3. Monthly Pool + Paygo Last Resort

Use this when several monthly accounts should behave like one preferred group, and paygo should only be used after that group is unavailable.

```toml
version = 4

[codex.providers.input]
base_url = "https://input.example/v1"
auth_token_env = "INPUT_API_KEY"
tags = { billing = "monthly" }

[codex.providers.input1]
base_url = "https://input1.example/v1"
auth_token_env = "INPUT1_API_KEY"
tags = { billing = "monthly" }

[codex.providers.input2]
base_url = "https://input2.example/v1"
auth_token_env = "INPUT2_API_KEY"
tags = { billing = "monthly" }

[codex.providers.codex_for]
base_url = "https://codex-for.example/v1"
auth_token_env = "CODEX_FOR_API_KEY"
tags = { billing = "paygo" }

[codex.routing]
entry = "monthly_first"

[codex.routing.routes.monthly_pool]
strategy = "ordered-failover"
children = ["input", "input1", "input2"]

[codex.routing.routes.monthly_first]
strategy = "ordered-failover"
children = ["monthly_pool", "codex_for"]
```

This is the cleanest expression of:

- try the monthly group first;
- let runtime health remove bad members temporarily;
- allow reprobe later;
- only fall through to paygo when the monthly branch cannot serve the request.

## 4. Tag-Preferred Monthly First

Use this when billing or region tags are the real intent and provider names should stay secondary.

```toml
version = 4

[codex.providers.monthly_a]
base_url = "https://monthly-a.example/v1"
auth_token_env = "MONTHLY_A_API_KEY"
tags = { billing = "monthly" }

[codex.providers.monthly_b]
base_url = "https://monthly-b.example/v1"
auth_token_env = "MONTHLY_B_API_KEY"
tags = { billing = "monthly" }

[codex.providers.paygo]
base_url = "https://paygo.example/v1"
auth_token_env = "PAYGO_API_KEY"
tags = { billing = "paygo" }

[codex.routing]
entry = "monthly_first"

[codex.routing.routes.monthly_first]
strategy = "tag-preferred"
prefer_tags = [{ billing = "monthly" }]
children = ["monthly_a", "monthly_b", "paygo"]
on_exhausted = "continue"
```

## 5. Strict Budget Stop

Use this when you do not want silent spillover into paygo.

```toml
[codex.routing.routes.monthly_first]
strategy = "tag-preferred"
prefer_tags = [{ billing = "monthly" }]
children = ["monthly_a", "monthly_b", "paygo"]
on_exhausted = "stop"
```

## 6. Future Region Split

Conditional routing is a future extension. Keep this as design intent, not a copy-pasteable v0.14.0 config.

```toml
[codex.routing]
entry = "root"

[codex.routing.routes.root]
strategy = "conditional"
when = { region = "eu" }
then = "eu_route"
default = "global_route"

[codex.routing.routes.eu_route]
strategy = "ordered-failover"
children = ["eu_provider", "global_backup"]

[codex.routing.routes.global_route]
strategy = "ordered-failover"
children = ["global_primary", "global_backup"]
```

## 7. Temporary Debug Pin

Use this when you want to freeze one provider for a short investigation.

```toml
[codex.routing.routes.debug_pin]
strategy = "manual-sticky"
target = "input1"
```

## What Not To Do

- Do not use route nodes just to hide unrelated providers under a confusing name.
- Do not infer monthly vs paygo from the provider key.
- Do not duplicate the same provider in several branches unless the compiler explicitly defines that behavior.
- Do not put runtime health rules into the config file.
