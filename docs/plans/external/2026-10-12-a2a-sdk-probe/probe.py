"""Drive the current a2a-sdk client against a Mycelium node, as Strands would."""
import asyncio, json, sys, uuid
import httpx
import a2a, importlib.metadata as md

BASE = sys.argv[1]

def show(label, r):
    body = r.text[:300].replace("\n", " ")
    print(f"{label}: HTTP {r.status_code} {body}")

async def main():
    print("a2a-sdk", md.version("a2a-sdk"))
    async with httpx.AsyncClient(timeout=10) as h:
        # 1. The card where the current SDK looks for it, and where Mycelium serves it.
        show("GET /.well-known/agent-card.json", await h.get(f"{BASE}/.well-known/agent-card.json"))
        legacy = await h.get(f"{BASE}/.well-known/agent.json")
        show("GET /.well-known/agent.json", legacy)

        # 2. The SDK's own resolver at its default path.
        from a2a.client.card_resolver import A2ACardResolver
        try:
            card = await A2ACardResolver(h, BASE).get_agent_card()
            print("resolver(default path): OK", card.name)
        except Exception as e:
            print("resolver(default path): FAILED", type(e).__name__, str(e)[:200])
        try:
            card = await A2ACardResolver(h, BASE, agent_card_path="/.well-known/agent.json").get_agent_card()
            print("resolver(legacy path): OK name=", card.name,
                  "interfaces=", [(i.url, i.protocol_binding if hasattr(i,'protocol_binding') else None, i.protocol_version) for i in card.supported_interfaces])
        except Exception as e:
            print("resolver(legacy path): FAILED", type(e).__name__, str(e)[:300])

        # 3. The JSON-RPC method names each spec generation sends, raw.
        msg_v03 = {"role": "user", "parts": [{"kind": "text", "text": "ping"}], "messageId": str(uuid.uuid4())}
        msg_v10 = {"role": "ROLE_USER", "parts": [{"text": "ping"}], "messageId": str(uuid.uuid4())}
        for method, params in [
            ("SendMessage", {"message": msg_v10}),             # A2A 1.0 (a2a-sdk 1.x default)
            ("message/send", {"message": msg_v03}),            # A2A 0.2/0.3 (a2a-sdk 0.3 compat)
            ("tasks/send", {"id": str(uuid.uuid4()), "skillId": "depot/dispatch", "message": {"role": "user", "parts": [{"type": "text", "text": "ping"}]}}),  # pre-0.2, what Mycelium serves
        ]:
            r = await h.post(f"{BASE}/a2a", json={"jsonrpc": "2.0", "id": 1, "method": method, "params": params})
            show(f"POST /a2a {method}", r)

asyncio.run(main())

async def sdk_client():
    """The path Strands takes: resolve the card, build a client from it, send a message."""
    from a2a.client.client_factory import ClientFactory, ClientConfig
    from a2a.client.card_resolver import A2ACardResolver
    async with httpx.AsyncClient(timeout=10) as h:
        for path in ("/.well-known/agent-card.json", "/.well-known/agent.json"):
            try:
                card = await A2ACardResolver(h, BASE, agent_card_path=path).get_agent_card()
            except Exception as e:
                print(f"sdk: card at {path}: FAILED {type(e).__name__}: {str(e)[:160]}")
                continue
            print(f"sdk: card at {path}: parsed name={card.name!r} interfaces={len(card.supported_interfaces)}")
            try:
                client = ClientFactory(ClientConfig(httpx_client=h)).create(card)
                print("sdk: client created", type(client).__name__)
            except Exception as e:
                print(f"sdk: client from card: FAILED {type(e).__name__}: {str(e)[:300]}")
            return

asyncio.run(sdk_client())
