import json, urllib.request
B = "http://localhost:8088/janus"
def post(url, body):
    body["transaction"] = "t"
    req = urllib.request.Request(url, json.dumps(body).encode(), {"Content-Type": "application/json"})
    return json.load(urllib.request.urlopen(req))
s = post(B, {"janus": "create"})["data"]["id"]
h = post(B + "/" + str(s), {"janus": "attach", "plugin": "janus.plugin.audiobridge"})["data"]["id"]
print(post(B + "/%d/%d" % (s, h), {"janus": "message", "body": {"request": "join", "room": 4815162342, "pin": "local-development-room-pin-change-me", "display": "probe", "codec": "opus", "rtp": {"ip": "127.0.0.1", "port": 12345, "payload_type": 100}}}))
for _ in range(2):
    print(json.dumps(json.load(urllib.request.urlopen(B + "/%d?maxev=1" % s))))