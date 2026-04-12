#!/bin/bash
echo "Initializing LocalStack infrastructure..."

# 1. Queue
awslocal sqs create-queue --queue-name transcode-queue-high
awslocal sqs create-queue --queue-name transcode-queue-low
awslocal sqs create-queue --queue-name segmentation-queue
awslocal sqs create-queue --queue-name playlist-queue.fifo --attributes FifoQueue=true

# 2. Create Bucket
awslocal s3 mb s3://video-uploads
# Configure Bucket for local traffic (testing)
awslocal s3api put-bucket-cors --bucket video-uploads --cors-configuration file:///etc/localstack/init/ready.d/cors.json

# 3. DynamoDB (Partition Key: video_id)
awslocal dynamodb create-table \
    --table-name videos \
    --attribute-definitions AttributeName=video_id,AttributeType=S \
    --key-schema AttributeName=video_id,KeyType=HASH \
    --billing-mode PAY_PER_REQUEST

echo "LocalStack initialization complete!"