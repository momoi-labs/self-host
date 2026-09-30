---
"self-host": patch
---

Let a restart or a redeploy pull newer images, so an Application on a moving
tag such as `:latest` can pick up a new release from the console. Restart opens
a confirmation with a "Pull newer images" checkbox, and Save and redeploy has
the same checkbox beside it. Both start from a new Platform setting and can be
changed each time. The event records the old and new image id of each pull,
and a failed pull before a restart leaves the containers running as they were.
Settings is now one page of cards that fills the screen instead of tabs.
