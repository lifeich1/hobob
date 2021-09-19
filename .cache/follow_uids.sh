#!/bin/bash

cat uids.txt | while read i; do 
    curl --location --request POST 'http://127.0.0.1:3000/op/follow' \
        --header 'Content-Type: application/json' \
        --data-raw "{
            \"enable\": true,
            \"uid\": $i
        }"
    echo "follow $i ..."
    sleep 1
done
