---
"self-host": minor
---

Connect application has a Database choice. By default the Platform creates a database named after the Application, such as `mochi_cut`, and the name can be edited. Existing database picks one the Platform already made on the server, so a second Application can share it and an Application can reach data an import or an earlier connection left there. New import can fill a new database with no Application connected, and Disconnect keeps the database for a later connection to pick. Connections made before this release keep their database and access as they were.
