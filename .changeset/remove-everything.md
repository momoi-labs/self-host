---
"self-host": patch
---

Removing an Application can now delete everything it stored. The console's Remove dialog has **Also delete everything it stored**, confirmed by typing the Application's name, and the API takes `?delete_data=true`: its named volumes, its project directory and data, its deployment history and the images only it used go with it. Without it, removal keeps the data as before.

`self-host prune` removes what removed Applications left behind, and Git builds no deployment keeps: volumes, project directories, images and deployment history. It lists each item with the space it frees and asks first. It is safe while everything runs: it only touches names that carry an Application id, and anything that still has a container stays. `GET /prune` shows the list and `POST /prune` runs it as a task.
