import asyncio, sys, httpx
from a2a.client.client_factory import ClientFactory, ClientConfig
from a2a.client.card_resolver import A2ACardResolver
from a2a.helpers.proto_helpers import new_text_message
from a2a.types import SendMessageRequest, Role
BASE = sys.argv[1]
async def main():
    async with httpx.AsyncClient(timeout=5) as h:
        card = await A2ACardResolver(h, BASE, agent_card_path="/.well-known/agent.json").get_agent_card()
        print("card interface url:", [i.url for i in card.supported_interfaces])
        client = ClientFactory(ClientConfig(httpx_client=h, streaming=False)).create(card)
        req = SendMessageRequest(message=new_text_message("dispatch one crate", role=Role.ROLE_USER))
        try:
            async for ev in client.send_message(req):
                print("sdk send_message: event", str(ev)[:200]); break
        except Exception as e:
            print("sdk send_message: FAILED", type(e).__name__, str(e)[:300])
asyncio.run(main())
