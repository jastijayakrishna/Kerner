set -eu
root="$1"
runtime="$2"
worker="$3"
mkdir -p "$root/checks"
if [ "$runtime" = runsc ]; then
  curl --max-time 60 -fsSL https://gvisor.dev/archive.key | sudo gpg --dearmor --yes -o /usr/share/keyrings/gvisor-archive-keyring.gpg
  printf 'deb [arch=%s signed-by=/usr/share/keyrings/gvisor-archive-keyring.gpg] https://storage.googleapis.com/gvisor/releases release main\n' "$(dpkg --print-architecture)" | sudo tee /etc/apt/sources.list.d/gvisor.list > /dev/null
  sudo apt-get update -qq
  sudo apt-get install -y -qq runsc > /dev/null
  sudo runsc install
  sudo systemctl restart docker
fi
real() { readlink -f "$(command -v "$1")"; }
hash() { sha256sum "$1" | cut -d' ' -f1; }
runtime_file="$(real "$runtime")"
printf '{"machine":"throwaway","host":"github-actions","experiment":"counterexample-v1"}\n' > "$root/host-policy.json"
jq -n --arg docker "$(real docker)" --arg git "$(real git)" --arg runtime "$runtime_file" --arg name "$runtime" \
  --arg version "$("$runtime_file" --version)" --arg dh "$(hash "$(real docker)")" --arg gh "$(hash "$(real git)")" \
  --arg rh "$(hash "$runtime_file")" --arg policy "$root/host-policy.json" --arg ph "$(hash "$root/host-policy.json")" \
  '{version:1,role:"verifier",docker:{file:$docker,sha256:$dh,endpoint:"unix:///var/run/docker.sock"},git:{file:$git,sha256:$gh},runtime:{file:$runtime,sha256:$rh,version:$version,arguments:[],name:$name},host_policy:{file:$policy,sha256:$ph}}' > "$root/control.json"
"$worker" --doctor "$root/control.json" > "$root/doctor.json"
docker pull -q docker.io/library/node:22-bookworm-slim@sha256:c3de60bf2f9dd0ac6370e6117950ff62d6e339527e7472301c9c78a017978392 > /dev/null
image="$(docker image inspect --format '{{.Id}}' docker.io/library/node:22-bookworm-slim@sha256:c3de60bf2f9dd0ac6370e6117950ff62d6e339527e7472301c9c78a017978392)"
printf '{"version":1,"cases":[{"command":["/usr/local/bin/npm","test"],"stdout":null}]}\n' > "$root/checks/probe.json"
jq -n --arg image "$image" --arg builder "$(jq -r .builder.sha256 "$root/doctor.json")" \
  --arg file "$root/checks/probe.json" --arg sha "$(hash "$root/checks/probe.json")" --arg repo "$REPO" \
  '{version:3,repository:$repo,deployment:"single-vm",executors:["external"],image:$image,builders:{executor:null,verifier:$builder},checks:[{id:"validation-probe",file:$file,sha256:$sha}],registry:"registry.npmjs.org",permissions:{}}' > "$root/operator.json"
