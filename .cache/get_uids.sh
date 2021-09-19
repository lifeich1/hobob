#!/bin/bash

cat << END | sqlite3 cache.db3 > uids.txt
select id from usersync;
END

