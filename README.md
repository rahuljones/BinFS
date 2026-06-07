[![Review Assignment Due Date](https://classroom.github.com/assets/deadline-readme-button-22041afd0340ce965d47ae6ef1cefeee28c7c493a6346c4f15d667ab976d596c.svg)](https://classroom.github.com/a/QRO06_15)
# CSE 223B Labs

Welcome to the CSE 223B Lab Series! 

- [CSE 223B Labs](#cse-223b-labs)
  - [Lab Workflow](#lab-workflow)
    - [How to Submit](#how-to-submit)
  - [Lab Setup](#lab-setup)
    - [Recommended Setup: Dev Container](#recommended-setup-dev-container)
    - [Alternative Setup: Manual Installation](#alternative-setup-manual-installation)
  - [Useful Links](#useful-links)

## Lab Workflow

We’ll use GitHub + [Gradescope](https://www.gradescope.com/courses/1289435) to access and submit assignments. Here’s how it works:

1. Get the assignment materials from GitHub classroom.
2. Clone the repository to any machine you are using.
3. Work on the assignment, pushing back to GitHub.
4. Submit the assignment on Gradescope.

Here is a diagram of the process ([credit](https://hmc-cs-131-spring2020.github.io/)): 
![image](https://hmc-cs-131-spring2020.github.io/assets/img/git-workflow.png)


### How to Submit

Your final solution need to be submitted to [Gradescope](https://www.gradescope.com/courses/1289435) to be graded. To submit your lab, complete the following steps:

1. The first time you submit, you may need to click “Connect to GitHub”, to connect your Gradescope account to your GitHub account.

2. Access the course’s Gradescope site, select the appropriate lab, and then choose GitHub as the submission method.

3. The first time you submit your repository, you will need to authorize Gradescope to access your git repository. Select the appropriate repository and branch.

4. You can submit multiple times before the deadline. Your last submission will determine your grade. You only will be able to see your grade on Gradescope after the deadline.

## Lab Setup

### Recommended Setup: Dev Container

We recommend using Visual Studio Code (VSCode) with *Dev Containers* extension.

1. Install the required software:
   - [Docker Desktop](https://www.docker.com/products/docker-desktop)
   - [Visual Studio Code](https://code.visualstudio.com/)
   - The **Dev Containers** extension for VSCode

2. Clone your Git repository to your local machine.

3. Open VSCode and launch the [Command Palette](https://code.visualstudio.com/docs/getstarted/userinterface#_command-palette):
   - Run the command: `Dev Containers: Open Folder in Container...`
   - Select the cloned repository folder.
   - VSCode will build and configure the container; this process may take several minutes.

4. Once the container is set up:
   - Open the Command Palette again and run: `Live Preview: Start Server`
   - This will launch the lab instructions in a new tab.

The container can be reused for future development work throughout the course.

### Alternative Setup: Manual Installation

If you prefer not to use Dev Containers, you can manually set up the environment:

1. Install Rust via [rustup](https://rustup.rs).

2. From the root of the repository, run:

   ```bash
   cargo doc
   ```
3. Open the generated documentation at `./target/doc/lab/index.html`.

## Useful Links

- Course website: [CSE 223B, Spring 2026: Distributed Computing and Systems](https://cseweb.ucsd.edu/classes/sp26/cse223B-a/)
- Course Gradescope: [Gradescope, CSE 223B - Distributed Computing&Systems - Snoeren [SP26]](https://www.gradescope.com/courses/1289435)


