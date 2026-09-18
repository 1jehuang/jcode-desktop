# Personal cloud access follow-up

Status: recommendations only, 2026-09-18. No provider credential was copied, no
model permission was expanded, and no non-root identity was enrolled by this
follow-up. Desktop's personal-alpha wake/default routing does not remove these
account setup requirements.

## Current blocker

The workstation's `jcode-personal` AWS browser-login session expired. Refresh it:

```sh
aws login --profile jcode-personal --region us-east-1
jcode-cloud-alpha status
jcode-cloud-alpha wake
```

Complete AWS sign-in and MFA yourself. Never paste a password or one-time code
into a Jcode conversation. The helper checks the configured account and host
ownership and refuses wake without the independent guard and available quota.

## A coding-capable model

The deployed role and wrapper currently permit/default to Nova Micro only.
Changing a Desktop model picker cannot override that IAM boundary.

The preferred credential-free host route is a suitable Claude Sonnet model on
Bedrock, using the existing instance role. The adjacent Jcode provider supports
Bedrock model IDs and US inference profiles. Before enabling it:

1. Explicitly approve the model's current inference pricing and applicable
   provider terms. The machine-hours guard is **not** a model-spend cap.
2. Verify this account's model availability, provider access prerequisites,
   inference-profile destinations and quota in the selected region. Do not
   assume catalog discovery implies invocation permission.
3. Narrowly authorize only the selected profile and its required destination
   foundation-model ARNs through reviewed infrastructure changes. Do not grant
   arbitrary `bedrock:*` or all-model invocation.
4. Update both the host's wrapper model environment and its Jcode provider/model
   config. The original `JCODE_BEDROCK_MODEL` wrapper still pins Nova Micro.
5. Verify the pinned remote Jcode release with one explicitly authorized small
   coding/tool turn and saved-history reconnect. Newer local provider support
   alone does not prove the older remote binary supports the selected model.

A separate-provider route is also possible with explicit remote authentication
and billing approval, but local login files, provider tokens, AWS credentials,
and SSH private keys must never be silently copied to the VM.

## Non-root operator

Prefer an AWS IAM Identity Center **organization instance**, an enrolled user
with MFA, and an account-assigned permission set. An application-only account
instance does not provide the needed AWS account access. Identity enrollment,
MFA, organization setup if absent, and permission assignment require explicit
operator setup and review. A manually enrolled console IAM user with
browser-based CLI login is an alternative. Do not create long-lived access keys
or invent a password as a shortcut.

The operator needs the helper's limited lifecycle permissions:

- `sts:GetCallerIdentity` for the account check.
- EC2 describe plus Start/Stop restricted to the personal-alpha instance.
- SSM instance information and StartSession for that instance and the approved
  `AWS-StartSSHSession` document, plus management of their own SSM sessions.
- Consistent DynamoDB GetItem on the guard ledger.
- EventBridge DescribeRule and Lambda GetFunctionConfiguration for the guard.

Describe actions without resource-level support may require `Resource: "*"`.
Review applicable region/account/tag conditions and exact service authorization
semantics before deploying a policy. Do not give the normal operator IAM,
provisioning, termination, guard-disable, or ledger-write permission. Keep
infrastructure administration a separate deliberate role.

After enrollment, configure a separate CLI profile, confirm its caller identity
is non-root in the recorded account, and test status/wake/SSH using that profile.
Only then update the local non-secret cloud-alpha profile setting. Keep an
explicit rollback path to the prior config, not exported root credentials.

Official references:

- [AWS CLI browser login](https://docs.aws.amazon.com/cli/latest/userguide/cli-configure-sign-in.html)
- [IAM Identity Center](https://docs.aws.amazon.com/singlesignon/latest/userguide/what-is.html)
- [STS credential caller comparison](https://docs.aws.amazon.com/IAM/latest/UserGuide/id_credentials_sts-comparison.html)
- [Bedrock model access](https://docs.aws.amazon.com/bedrock/latest/userguide/model-access.html)

Root-authenticated CLI sessions are not a substitute for non-root enrollment.
Do not claim that wrapping the current root login in a profile or unverified STS
call has made the operator scoped.
