# Cedar: an introduction (script)

Running time 3:53.6. Times are from `timeline.json` of the take in `cedar-intro.mp4`.
Each scene is at least as long as its narration at about 150 words a minute, so the voice fits when each
clip starts with its scene.

| # | Scene | Starts | Ends | Narration |
| --- | --- | ---: | ---: | ---: |
| 1 | Title card | 0:00.4 | 0:09.3 | 19 words, about 8 s |
| 2 | First start | 0:09.3 | 0:21.5 | 27 words, about 11 s |
| 3 | Adding a model | 0:21.5 | 1:04.3 | 97 words, about 40 s |
| 4 | Giving it a task | 1:04.3 | 1:20.6 | 33 words, about 13 s |
| 5 | The plan | 1:20.6 | 1:36.5 | 33 words, about 13 s |
| 6 | Working in a sealed box | 1:36.5 | 2:01.4 | 53 words, about 22 s |
| 7 | The result | 2:01.4 | 2:26.5 | 34 words, about 14 s |
| 8 | Asking first | 2:26.5 | 2:42.0 | 35 words, about 14 s |
| 9 | Memory | 2:42.0 | 3:04.8 | 35 words, about 14 s |
| 10 | A switch for every ability | 3:04.8 | 3:18.2 | 22 words, about 9 s |
| 11 | From conversation to agent | 3:18.2 | 3:40.9 | 42 words, about 17 s |
| 12 | Closing card | 3:40.9 | 3:53.5 | 28 words, about 11 s |

## 1. Title card (0:00.4 to 0:09.3)

**On screen.** Title card: the Cedar mark, "Your own assistant. Runs at home, answers to you."

**Narration.** This is Cedar: an open source assistant that runs on your own computer, and does real work for you.

## 2. First start (0:09.3 to 0:21.5)

**On screen.** The chat opens for the first time. The language dialog; the pointer passes over Deutsch, scrolls, and continues with English. The welcome page says no model is registered yet.

**Narration.** The first time you open it, Cedar asks which language you'd like to work in. After that it needs just one thing: a model to think with.

## 3. Adding a model (0:21.5 to 1:04.3)

**On screen.** More → Settings → Models → Add a model. The three choices: run one here with llama.cpp, a hosted provider, a server you already run. The third opens; the vLLM preset fills in the address; List models; the model is chosen and named; the reasoning levels and "Images: no" appear; default for new chats; Check and add. The model is listed.

**Narration.** Cedar isn't tied to one AI company. Under Settings, Models, you choose where the model comes from. The Cedar app for Mac and Windows can download one and run it for you with llama.cpp. You can use a hosted provider with an API key. Or you can point Cedar at a model server you already run. Here, that's vLLM on this machine. Cedar lists the models the server offers. Pick one, and it fills in the reasoning levels the model takes, and whether it can read images. Then it checks the address once more, and saves it.

## 4. Giving it a task (1:04.3 to 1:20.6)

**On screen.** Back on the welcome page. household-spending-2026.csv is attached and the request is typed.

**Narration.** Now for a task. I'll attach a year of household spending, and ask Cedar to work out where the money goes, chart it, and write a one-page summary with three ways to save.

## 5. The plan (1:20.6 to 1:36.5)

**On screen.** The request is sent. The plan card appears with five steps; the pointer passes over the steps, a remove button, Add a step, then clicks "Looks good, start".

**Narration.** Cedar doesn't just start. For a bigger task, it shows you its plan first. You can reword a step, remove one, or add your own. When it looks right, tell it to start.

## 6. Working in a sealed box (1:36.5 to 2:01.4)

**On screen.** The status line reads Working. On the right a Python step lands, then the chart card, then the PDF. The plan shows 5/5 done. The Python step is opened: the code and what it printed.

**Narration.** The work happens in a sealed box: a container of its own, with the network off, that can't see the rest of your computer. Cedar runs the numbers in Python, draws the chart, and writes the PDF. Each step stays in the conversation, so you can open it and see exactly what ran.

## 7. The result (2:01.4 to 2:26.5)

**On screen.** The chart card: the pointer moves across the months and the tooltip lists every category. Then the PDF summary scrolls by: the table and the three ways to save. The conversation scrolls to Cedar's answer.

**Narration.** The chart is interactive: point at a month to read every category. The finished summary sits on the right, with the three suggestions, ready to download. And the short version is in the conversation.

## 8. Asking first (2:26.5 to 2:42.0)

**On screen.** A follow-up is typed: delete the original CSV. A "Needs your OK · delete files" card shows the command. Allow once. Cedar reports the file is gone.

**Narration.** Some things Cedar won't do on its own. Ask it to delete a file, and it stops and asks first. Downloads, installs and sending email work the same way. Nothing risky happens without your OK.

## 9. Memory (2:42.0 to 3:04.8)

**On screen.** "Remember that we want to keep our spending under 2,800 euros a month." Cedar notes it. Settings → Memory shows the remembered preference with Pin, Archive and Remove.

**Narration.** Cedar also remembers what you ask it to. Under Settings, Memory, you see everything it has kept about you. Pin it, archive it, or remove it for good. It stays on your own Cedar server.

## 10. A switch for every ability (3:04.8 to 3:18.2)

**On screen.** Settings → Tools. The pointer runs down the switches: web pages, weather, maps, directions; Stocks & markets is switched off.

**Narration.** Every ability has a switch. Reading web pages, weather, maps, directions, stocks. Turn on what Cedar may reach. Everything else stays off.

## 11. From conversation to agent (3:18.2 to 3:40.9)

**On screen.** The "Make an agent from this chat" button. The New agent page with the prompt written from the conversation; it is named "Monthly spending check"; the pointer passes over what can start it; How often is set to Every month; Create agent. The agent's page shows its next run.

**Narration.** And when a conversation did its job, turn it into an agent. Cedar writes the prompt from the conversation. You choose what starts it: a schedule, a change it watches for, new mail, a file arriving. This one will run every month.

## 12. Closing card (3:40.9 to 3:53.5)

**On screen.** Closing card: the Cedar mark, the tagline, Mac · Windows · Linux · iPhone · Android, github.com/hanneshapke/cedar.

**Narration.** Cedar is free and open source. It runs on Mac, Windows and Linux, with apps for iPhone and Android. Your own assistant. Runs at home, answers to you.
