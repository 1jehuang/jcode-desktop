#!/usr/bin/env python3
"""Generate reviewable CloudFormation for ONE personal alpha, never a customer fleet."""
import argparse
import base64
import json
from pathlib import Path
import re

HERE = Path(__file__).resolve().parent


def ref(name):
    return {"Ref": name}


def sub(value):
    return {"Fn::Sub": value}


def arn(name):
    return {"Fn::GetAtt": [name, "Arn"]}


def policy(statements):
    return {"Version": "2012-10-17", "Statement": statements}


def allow(actions, resources):
    return {"Effect": "Allow", "Action": actions, "Resource": resources}


def role(service, statements, managed=()):
    return {"Type": "AWS::IAM::Role", "Properties": {
        "AssumeRolePolicyDocument": policy([{
            "Effect": "Allow", "Principal": {"Service": service}, "Action": "sts:AssumeRole"}]),
        "ManagedPolicyArns": list(managed),
        "Policies": [{"PolicyName": "alpha-only", "PolicyDocument": policy(statements)}] if statements else [],
    }}


def generate(public_key):
    # Key only, no comment or shell syntax ever interpolated into the bootstrap.
    parts = public_key.strip().split()
    if len(parts) < 2 or parts[0] != "ssh-ed25519" or not re.fullmatch(r"[A-Za-z0-9+/]+={0,2}", parts[1]):
        raise ValueError("An OpenSSH ed25519 public key is required")
    base64.b64decode(parts[1], validate=True)
    key = " ".join(parts[:2])
    bootstrap = (HERE / "bootstrap.sh").read_text().replace("__SSH_PUBLIC_KEY__", key)
    if (HERE / "idle.py").exists():
        idle = base64.b64encode((HERE / "idle.py").read_bytes()).decode()
        bootstrap = bootstrap.replace("touch /opt/jcode-alpha/ready", f"""printf '%s' '{idle}' | base64 -d > /opt/jcode-alpha/idle.py
cat > /etc/systemd/system/jcode-alpha-idle.service <<'EOF'
[Unit]
Description=Conservative Jcode alpha idle check
[Service]
Type=oneshot
ExecStart=/usr/bin/python3 /opt/jcode-alpha/idle.py
TimeoutStartSec=45
EOF
cat > /etc/systemd/system/jcode-alpha-idle.timer <<'EOF'
[Unit]
Description=Jcode alpha idle stop
[Timer]
OnBootSec=5min
OnUnitActiveSec=1min
AccuracySec=5s
[Install]
WantedBy=timers.target
EOF
systemctl daemon-reload
systemctl enable --now jcode-alpha-idle.timer
touch /opt/jcode-alpha/ready""")
    # EC2's decoded user-data limit is 16 KiB. gzip is supported by cloud-init.
    import gzip
    user_data = base64.b64encode(gzip.compress(bootstrap.encode(), mtime=0)).decode()
    if len(base64.b64decode(user_data)) > 16384:
        raise ValueError("Compressed cloud-init exceeds EC2 limit")
    instance_arn = sub("arn:${AWS::Partition}:ec2:${AWS::Region}:${AWS::AccountId}:instance/${Host}")
    tags = [{"Key": "Project", "Value": "jcode-cloud-alpha"}, {"Key": "Name", "Value": "jcode-cloud-alpha"}]
    resources = {
        "HostRole": role("ec2.amazonaws.com", [
            allow(["bedrock:InvokeModel", "bedrock:InvokeModelWithResponseStream"], [
                sub("arn:${AWS::Partition}:bedrock:${AWS::Region}::foundation-model/amazon.nova-micro-v1:0")]),
            allow(["bedrock:ListFoundationModels", "bedrock:ListInferenceProfiles"], "*"),
        ], [sub("arn:${AWS::Partition}:iam::aws:policy/AmazonSSMManagedInstanceCore")]),
        "HostProfile": {"Type": "AWS::IAM::InstanceProfile", "Properties": {"Roles": [ref("HostRole")]}},
        "SecurityGroup": {"Type": "AWS::EC2::SecurityGroup", "Properties": {
            "GroupDescription": "Jcode personal alpha: no inbound, SSM tunnel only",
            "VpcId": ref("VpcId"), "SecurityGroupIngress": [],
            "SecurityGroupEgress": [{"IpProtocol": "-1", "CidrIp": "0.0.0.0/0"}], "Tags": tags}},
        "Host": {"Type": "AWS::EC2::Instance", "DeletionPolicy": "Retain", "UpdateReplacePolicy": "Retain", "Properties": {
            "ImageId": ref("ImageId"), "InstanceType": "m7i.large",
            "IamInstanceProfile": ref("HostProfile"),
            "InstanceInitiatedShutdownBehavior": "stop", "DisableApiTermination": True,
            "MetadataOptions": {"HttpTokens": "required", "HttpPutResponseHopLimit": 1},
            "NetworkInterfaces": [{"DeviceIndex": "0", "AssociatePublicIpAddress": True,
                "SubnetId": ref("SubnetId"), "GroupSet": [ref("SecurityGroup")]}],
            "BlockDeviceMappings": [{"DeviceName": "/dev/xvda", "Ebs": {
                "VolumeSize": 30, "VolumeType": "gp3", "Encrypted": True, "DeleteOnTermination": False}}],
            "UserData": user_data, "Tags": tags}},
        "Ledger": {"Type": "AWS::DynamoDB::Table", "DeletionPolicy": "Retain", "UpdateReplacePolicy": "Retain", "Properties": {
            "BillingMode": "PAY_PER_REQUEST", "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}, {"AttributeName": "sk", "AttributeType": "S"}],
            "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}, {"AttributeName": "sk", "KeyType": "RANGE"}], "SSESpecification": {"SSEEnabled": True}, "Tags": tags}},
        "GuardRole": role("lambda.amazonaws.com", [
            allow("ec2:DescribeInstances", "*"), allow("ec2:StopInstances", instance_arn),
            allow(["dynamodb:GetItem", "dynamodb:PutItem", "dynamodb:UpdateItem", "dynamodb:TransactWriteItems"], arn("Ledger")),
            allow(["logs:CreateLogStream", "logs:PutLogEvents"], sub("arn:${AWS::Partition}:logs:${AWS::Region}:${AWS::AccountId}:log-group:/aws/lambda/${AWS::StackName}-guard:*")),
        ]),
        "GuardLog": {"Type": "AWS::Logs::LogGroup", "Properties": {
            "LogGroupName": sub("/aws/lambda/${AWS::StackName}-guard"), "RetentionInDays": 7}},
        "Guard": {"Type": "AWS::Lambda::Function", "DependsOn": "GuardLog", "Properties": {
            "FunctionName": sub("${AWS::StackName}-guard"), "Runtime": "python3.12", "Handler": "index.handler",
            "Role": arn("GuardRole"), "Timeout": 45, "MemorySize": 128,
            "Environment": {"Variables": {"INSTANCE_ID": ref("Host"), "TABLE_NAME": ref("Ledger"),
                "MONTHLY_HOURS": "50", "MAX_BOOT_HOURS": "2"}},
            "Code": {"ZipFile": (HERE / "guard.py").read_text()}}},
        "GuardSchedule": {"Type": "AWS::Events::Rule", "Properties": {
            "ScheduleExpression": "rate(1 minute)", "State": "ENABLED",
            "Targets": [{"Arn": arn("Guard"), "Id": "guard"}]}},
        "SchedulePermission": {"Type": "AWS::Lambda::Permission", "Properties": {
            "Action": "lambda:InvokeFunction", "FunctionName": ref("Guard"), "Principal": "events.amazonaws.com",
            "SourceArn": arn("GuardSchedule")}},
    }
    return {"AWSTemplateFormatVersion": "2010-09-09",
        "Description": "Single-user Jcode Cloud alpha. Persistent 30GB, no ingress, 2h boot lease, 50h/month guard.",
        "Parameters": {
            "VpcId": {"Type": "AWS::EC2::VPC::Id"}, "SubnetId": {"Type": "AWS::EC2::Subnet::Id"},
            "ImageId": {"Type": "AWS::EC2::Image::Id"}},
        "Resources": resources,
        "Outputs": {"InstanceId": {"Value": ref("Host")}, "LedgerTable": {"Value": ref("Ledger")},
            "GuardFunction": {"Value": ref("Guard")}, "GuardRule": {"Value": ref("GuardSchedule")}, "SecurityGroupId": {"Value": ref("SecurityGroup")}}}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--public-key", type=Path, required=True)
    args = parser.parse_args()
    print(json.dumps(generate(args.public_key.read_text()), indent=2))
