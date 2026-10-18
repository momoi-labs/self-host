---
"self-host": minor
---

An uploaded database import now resumes. The console sends the dump in chunks and the Host keeps every byte that arrived, so Retry after a dropped connection sends only the rest. After a page refresh, choose the same file again in New import and the upload goes on from where it stopped; the Imports list shows its progress from the bytes already on the Host. Another file starts over. An upload left unfinished for a day is removed, file included, and Dismiss removes it at once.
