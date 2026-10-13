---
"self-host": minor
---

An image Application can enable a deploy trigger from its Deployments tab, like a Git Application. CI calls `POST /deploy/{id}` with the trigger token after pushing to the image's registry repository, naming the new tag in `image` or leaving it out to pull the configured reference again. The trigger binds to the repository, so a different tag deploys and a different repository is refused.
